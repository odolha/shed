package dev.shed

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import dev.shed.ui.Nav
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.signin.SignInScreen
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.ShedTheme
import kotlinx.coroutines.flow.MutableStateFlow

class MainActivity : FragmentActivity() {
    /** A `shed://` link to open (from a notification), until the screens take it. */
    val link = MutableStateFlow<Uri?>(null)

    /** The app is locked behind the fingerprint (or the screen lock). */
    val locked = MutableStateFlow(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        // A link opens once: not again when the activity is made anew (rotation).
        if (savedInstanceState == null) link.value = intent?.data
        locked.value = shed.settings.biometricLock && canUnlock()
        setContent { Root(this) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.data?.let { link.value = it }
    }

    override fun onStop() {
        super.onStop()
        if (shed.settings.biometricLock && !isChangingConfigurations && canUnlock()) locked.value = true
    }

    fun canUnlock(): Boolean =
        BiometricManager.from(this).canAuthenticate(BIOMETRIC_STRONG or DEVICE_CREDENTIAL) == BiometricManager.BIOMETRIC_SUCCESS

    /** Asks for the fingerprint (or the screen lock); [done] with whether it was given. */
    fun authenticate(done: (Boolean) -> Unit) {
        val prompt = BiometricPrompt(
            this,
            ContextCompat.getMainExecutor(this),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) = done(true)
                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) = done(false)
            },
        )
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(getString(R.string.lock_title))
            .setAllowedAuthenticators(BIOMETRIC_STRONG or DEVICE_CREDENTIAL)
            .build()
        prompt.authenticate(info)
    }

    fun unlock() = authenticate { if (it) locked.value = false }
}

@Composable
private fun Root(activity: MainActivity) {
    val app = activity.shed
    val theme by app.theme.collectAsState()
    val session by app.session.collectAsState()
    val locked by activity.locked.collectAsState()
    val snackbar = remember { SnackbarHostState() }
    ShedTheme(theme) {
        CompositionLocalProvider(LocalSnackbar provides snackbar) {
            Surface(Modifier.fillMaxSize(), color = LocalShed.current.background) {
                when {
                    locked -> Locked(activity)
                    session == null -> SignInScreen()
                    else -> Nav(activity.link)
                }
            }
        }
    }
}

@Composable
private fun Locked(activity: MainActivity) {
    LaunchedEffect(Unit) { activity.unlock() }
    Column(
        Modifier.fillMaxSize(),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(Icons.Default.Fingerprint, null, Modifier.size(64.dp), tint = LocalShed.current.accent)
        Text(stringResource(R.string.lock_title))
        Button(onClick = { activity.unlock() }) { Text(stringResource(R.string.lock_unlock)) }
    }
}
