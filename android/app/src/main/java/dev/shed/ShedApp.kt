package dev.shed

import android.app.Application
import android.content.Context
import android.os.SystemClock
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import dev.shed.api.ErrorCode
import dev.shed.data.Account
import dev.shed.data.AccountStore
import dev.shed.data.Loaded
import dev.shed.data.Settings
import dev.shed.data.ThemeChoice
import dev.shed.lock.AppLock
import dev.shed.lock.LockAfter
import dev.shed.net.Connection
import dev.shed.net.Empty
import dev.shed.net.NexusHttp
import dev.shed.net.NexusSocket
import dev.shed.push.Push
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import java.util.concurrent.TimeUnit

/** A signed-in phone's way to the nexus: HTTP and the WebSocket. */
class Session(val account: Account, val http: NexusHttp, val socket: NexusSocket)

class ShedApp : Application() {
    /** Lives as long as the process. */
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    lateinit var accounts: AccountStore
        private set
    lateinit var settings: Settings
        private set

    /** The app lock: nothing of the workspaces is shown or fetched while it is locked. */
    lateinit var lock: AppLock
        private set

    val client: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .pingInterval(30, TimeUnit.SECONDS)
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .build()
    }

    private val _session = MutableStateFlow<Session?>(null)
    val session: StateFlow<Session?> = _session.asStateFlow()

    private val _theme = MutableStateFlow(ThemeChoice.Dark)
    val theme: StateFlow<ThemeChoice> = _theme.asStateFlow()

    private var foreground = false
    private var stopJob: Job? = null
    private var watchJob: Job? = null

    override fun onCreate() {
        super.onCreate()
        accounts = AccountStore(this)
        settings = Settings(this)
        lock = AppLock(settings.lockAfter, SystemClock::elapsedRealtime, ::canUnlock)
        _theme.value = settings.theme
        Push.createChannels(this)
        // Locked: the account is read (and connected) only after the unlock.
        lock.coldStart(accounts.has())
        if (!lock.locked.value) loadAccount()
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                foreground = true
                stopJob?.cancel()
                if (lock.foreground()) {
                    // Away too long: locked until the unlock, offline meanwhile.
                    _session.value?.socket?.stop()
                    return
                }
                // The screen lock went while away: read the account without it.
                if (_session.value == null && accounts.has()) loadAccount()
                connect()
            }

            override fun onStop(owner: LifecycleOwner) {
                foreground = false
                lock.background()
                // Out of view: notifications are pushed (or go live to another
                // app in view); the socket can rest a while after.
                _session.value?.socket?.setVisible(false)
                stopJob = scope.launch {
                    delay(60_000)
                    _session.value?.socket?.stop()
                }
            }
        })
    }

    /** Whether the phone has a screen lock (or a strong biometric) to unlock with. */
    fun canUnlock(): Boolean =
        BiometricManager.from(this).canAuthenticate(BIOMETRIC_STRONG or DEVICE_CREDENTIAL) == BiometricManager.BIOMETRIC_SUCCESS

    private fun inFront(): Boolean =
        foreground || ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)

    private fun connect() {
        val socket = _session.value?.socket ?: return
        if (lock.locked.value || !inFront()) return
        socket.setVisible(true)
        socket.start()
        socket.retryNow()
    }

    /**
     * The phone's owner just unlocked (the prompt succeeded): reads the
     * account (its token under the locked key works only now), moving a
     * token kept the old way under that key, and connects.
     */
    fun unlocked() {
        lock.unlocked()
        if (_session.value == null) {
            loadAccount()
        } else {
            accounts.reseal(lock.active)
        }
        connect()
    }

    /** Reads the kept account and opens it; a key lost (the screen lock removed) signs out here. */
    private fun loadAccount() {
        when (val loaded = accounts.load()) {
            is Loaded.Found -> {
                // Kept the old way, or the lock turned on or off: under the key it should be.
                accounts.reseal(lock.active)
                open(loaded.account)
            }
            Loaded.KeyLost -> {
                Push.unregister(this)
                accounts.wipe(getString(R.string.signed_out_key_lost))
            }
            Loaded.None, Loaded.NotUnlocked -> {}
        }
    }

    /** Changes when the app locks; called after an unlock, so the token can move to its key. */
    fun setLockAfter(after: LockAfter) {
        settings.lockAfter = after
        lock.set(after)
        accounts.reseal(lock.active)
    }

    fun setTheme(choice: ThemeChoice) {
        settings.theme = choice
        _theme.value = choice
    }

    /**
     * Signed in: keeps the account and connects. With the lock on, the token
     * goes under the locked key if the phone was unlocked just now, else at
     * the next unlock (MainActivity asks for one right after signing in).
     */
    fun signedIn(account: Account) {
        accounts.save(account, lock.active)
        open(account)
    }

    private fun open(account: Account) {
        _session.value?.socket?.stop()
        val http = NexusHttp(client, account.nexus, ready = { lock.awaitUnlocked() }) { account.token }
        val socket = NexusSocket(client, account.nexus, account.token, scope, "android ${BuildConfig.VERSION_NAME}")
        val session = Session(account, http, socket)
        _session.value = session
        watchJob?.cancel()
        watchJob = scope.launch {
            socket.connection.collect {
                if (it is Connection.Closed && it.error.code == ErrorCode.Unauthorized) forget(it.error.message)
            }
        }
        connect()
        Push.refresh(this)
        Push.sendPendingEndpoint(this)
    }

    /** Signs out at the nexus (best effort) and forgets everything. */
    suspend fun signOut() {
        val session = _session.value ?: return
        runCatching { session.http.post("/logout", Empty.serializer()) }
        forget()
    }

    /**
     * Forgets the account here (signed out, or revoked elsewhere), keeping
     * the nexus address and user name for the next sign-in, and [why], the
     * nexus's words, when it signed this phone out (`signed out: not used
     * for 7 days`).
     */
    fun forget(why: String? = null) {
        val session = _session.value ?: return
        watchJob?.cancel()
        session.socket.stop()
        Push.unregister(this)
        accounts.wipe(why?.let(::signedOutReason))
        _session.value = null
    }
}

val Context.shed: ShedApp get() = applicationContext as ShedApp

/**
 * Why the nexus signed this phone out, from its words (`signed out: not
 * used for 7 days` gives `not used for 7 days`); null for a plain `signed
 * out` (this phone's own sign-out). A token the nexus does not know at all
 * (`not signed in`) says so.
 */
fun signedOutReason(message: String): String? {
    val text = message.trim()
    return when {
        text.startsWith("signed out:") -> text.removePrefix("signed out:").trim().ifEmpty { null }
        text == "signed out" -> null
        else -> text.ifEmpty { null }
    }
}
