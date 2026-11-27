package dev.shed

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import dev.shed.lock.LockAfter
import dev.shed.ui.Nav
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.lock.LockScreen
import dev.shed.ui.signin.SignInScreen
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.ShedTheme
import kotlinx.coroutines.flow.MutableStateFlow

class MainActivity : FragmentActivity() {
    /** A `shed://` link to open (from a notification), until the screens take it (after the unlock). */
    val link = MutableStateFlow<Uri?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        // A link opens once: not again when the activity is made anew (rotation).
        if (savedInstanceState == null) link.value = intent?.data
        hideFromRecents()
        setContent { Root(this) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.data?.let { link.value = it }
    }

    /**
     * While the lock is on, the task switcher shows no picture of the app.
     * Android 13 and later have a switch for just that; before, the window
     * is made secure while the app is not in front (when the picture is
     * taken), so screenshots in the app still work.
     */
    fun hideFromRecents() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) setRecentsScreenshotEnabled(!shed.lock.active)
    }

    override fun onPause() {
        super.onPause()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU && shed.lock.active) {
            window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        }
    }

    override fun onResume() {
        super.onResume()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }

    fun canUnlock(): Boolean = shed.canUnlock()

    /** Asks for the fingerprint, face or screen lock; [done] with whether it was given. */
    fun authenticate(title: Int = R.string.lock_title, done: (Boolean) -> Unit) {
        if (!canUnlock()) return done(false)
        val lock = shed.lock
        lock.unlocking(true)
        val prompt = BiometricPrompt(
            this,
            ContextCompat.getMainExecutor(this),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    lock.unlocking(false)
                    done(true)
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    lock.unlocking(false)
                    done(false)
                }
            },
        )
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(getString(title))
            .setAllowedAuthenticators(BIOMETRIC_STRONG or DEVICE_CREDENTIAL)
            .build()
        prompt.authenticate(info)
    }

    /** The lock screen's Unlock: the session opens (and connects) once given. */
    fun unlock() {
        // No screen lock (removed meanwhile): nothing to ask for.
        if (!canUnlock()) return shed.unlocked()
        authenticate { if (it) shed.unlocked() }
    }

    /** Just signed in with the lock on: one unlock puts the token under the locked key. */
    fun protectSignIn() {
        if (!shed.lock.active || shed.accounts.isLocked()) return
        authenticate(R.string.lock_protect_title) { ok -> if (ok) shed.unlocked() }
    }

    /** Changes when the app locks; turning it off (or on) asks for an unlock first. */
    fun setLockAfter(after: LockAfter, done: (Boolean) -> Unit = {}) {
        val app = shed
        val needsUnlock = after == LockAfter.Off || app.settings.lockAfter == LockAfter.Off
        val apply = {
            app.setLockAfter(after)
            hideFromRecents()
            done(true)
        }
        if (!needsUnlock) return apply()
        authenticate { ok -> if (ok) apply() else done(false) }
    }
}

@Composable
private fun Root(activity: MainActivity) {
    val app = activity.shed
    val theme by app.theme.collectAsState()
    val session by app.session.collectAsState()
    val locked by app.lock.locked.collectAsState()
    val snackbar = remember { SnackbarHostState() }
    val focus = LocalFocusManager.current
    // Locked: no field under the lock keeps the keyboard.
    LaunchedEffect(locked) { if (locked) focus.clearFocus(force = true) }
    ShedTheme(theme) {
        CompositionLocalProvider(LocalSnackbar provides snackbar) {
            Surface(Modifier.fillMaxSize(), color = LocalShed.current.background) {
                Box(Modifier.fillMaxSize()) {
                    // Locked again after a while away, the screens stay as they were
                    // (a photo being picked comes back to them) but draw nothing and
                    // say nothing until the unlock.
                    val hidden = if (locked) Modifier.drawWithContent {}.clearAndSetSemantics {} else Modifier
                    Box(Modifier.fillMaxSize().then(hidden)) {
                        when {
                            session != null -> Nav(activity.link)
                            !locked -> SignInScreen()
                        }
                    }
                    if (locked) {
                        LaunchedEffect(Unit) { activity.unlock() }
                        LockScreen(onUnlock = activity::unlock)
                    }
                }
            }
        }
    }
}
