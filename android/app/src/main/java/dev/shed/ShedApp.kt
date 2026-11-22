package dev.shed

import android.app.Application
import android.content.Context
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import dev.shed.api.ErrorCode
import dev.shed.data.Account
import dev.shed.data.AccountStore
import dev.shed.data.Settings
import dev.shed.data.ThemeChoice
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
        _theme.value = settings.theme
        Push.createChannels(this)
        accounts.load()?.let { open(it) }
        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) {
                foreground = true
                stopJob?.cancel()
                _session.value?.socket?.let {
                    it.setVisible(true)
                    it.start()
                    it.retryNow()
                }
            }

            override fun onStop(owner: LifecycleOwner) {
                foreground = false
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

    fun setTheme(choice: ThemeChoice) {
        settings.theme = choice
        _theme.value = choice
    }

    /** Signed in: keeps the account and connects. */
    fun signedIn(account: Account) {
        accounts.save(account)
        open(account)
    }

    private fun open(account: Account) {
        _session.value?.socket?.stop()
        val http = NexusHttp(client, account.nexus) { account.token }
        val socket = NexusSocket(client, account.nexus, account.token, scope, "android ${BuildConfig.VERSION_NAME}")
        val session = Session(account, http, socket)
        _session.value = session
        watchJob?.cancel()
        watchJob = scope.launch {
            socket.connection.collect {
                if (it is Connection.Closed && it.error.code == ErrorCode.Unauthorized) forget()
            }
        }
        if (foreground || ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(androidx.lifecycle.Lifecycle.State.STARTED)) {
            socket.setVisible(true)
            socket.start()
        }
        Push.refresh(this)
    }

    /** Signs out at the nexus (best effort) and forgets everything. */
    suspend fun signOut() {
        val session = _session.value ?: return
        runCatching { session.http.post("/logout", Empty.serializer()) }
        forget()
    }

    /** Forgets the account here (signed out, or revoked elsewhere). */
    fun forget() {
        val session = _session.value ?: return
        watchJob?.cancel()
        session.socket.stop()
        Push.unregister(this)
        accounts.wipe()
        _session.value = null
    }
}

val Context.shed: ShedApp get() = applicationContext as ShedApp
