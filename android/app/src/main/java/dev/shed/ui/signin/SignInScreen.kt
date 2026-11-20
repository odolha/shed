package dev.shed.ui.signin

import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.foundation.Image
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import dev.shed.R
import dev.shed.api.DeviceHello
import dev.shed.api.DeviceKind
import dev.shed.api.ErrorCode
import dev.shed.api.Login
import dev.shed.api.NexusStatus
import dev.shed.api.SignedIn
import dev.shed.data.Account
import dev.shed.net.ApiException
import dev.shed.net.NexusHttp
import dev.shed.shed
import dev.shed.ui.common.errorText
import dev.shed.ui.theme.LocalShed
import kotlinx.coroutines.launch

/** Signing in to a nexus: its address, the account, and a code from the authenticator. */
@Composable
fun SignInScreen() {
    val context = LocalContext.current
    val app = context.shed
    val shed = LocalShed.current
    val scope = rememberCoroutineScope()
    var nexus by rememberSaveable { mutableStateOf(app.accounts.lastNexus()) }
    var username by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    var totp by rememberSaveable { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun signIn() {
        val url = NexusHttp.normalizeUrl(nexus)
        if (url == null) {
            error = context.getString(R.string.signin_bad_url)
            return
        }
        busy = true
        error = null
        scope.launch {
            try {
                val http = NexusHttp(app.client, url) { null }
                val status = http.get("/status", NexusStatus.serializer())
                if (!status.ready) throw ApiException.of(ErrorCode.Invalid, context.getString(R.string.signin_not_set_up))
                val login = Login(username.trim(), password, totp.trim(), DeviceHello(DeviceKind.Phone, Build.MODEL ?: "Android"))
                val signed = http.post("/login", login, Login.serializer(), SignedIn.serializer())
                val token = signed.token ?: throw ApiException.of(ErrorCode.Failed, "no device token")
                app.accounts.rememberNexus(url)
                app.signedIn(Account(url, signed.username, signed.device, token))
            } catch (e: Exception) {
                if (e is kotlinx.coroutines.CancellationException) throw e
                error = errorText(context, e)
                totp = ""
            } finally {
                busy = false
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .systemBarsPadding()
            .imePadding()
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
    ) {
        Image(painterResource(R.drawable.ic_launcher_foreground), null, Modifier.size(120.dp))
        Text(stringResource(R.string.signin_title), style = MaterialTheme.typography.headlineSmall)
        Text(
            stringResource(R.string.signin_intro),
            style = MaterialTheme.typography.bodyMedium,
            color = shed.textDim,
            modifier = Modifier.widthIn(max = 420.dp),
        )
        val field = Modifier.fillMaxWidth().widthIn(max = 420.dp)
        OutlinedTextField(
            value = nexus,
            onValueChange = { nexus = it },
            label = { Text(stringResource(R.string.signin_nexus)) },
            placeholder = { Text("https://nexus.example.com") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
            modifier = field,
        )
        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            label = { Text(stringResource(R.string.signin_username)) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next),
            modifier = field,
        )
        OutlinedTextField(
            value = password,
            onValueChange = { password = it },
            label = { Text(stringResource(R.string.signin_password)) },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Next),
            modifier = field,
        )
        OutlinedTextField(
            value = totp,
            onValueChange = { totp = it.take(32) },
            label = { Text(stringResource(R.string.signin_totp)) },
            supportingText = { Text(stringResource(R.string.signin_totp_hint)) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            modifier = field,
        )
        error?.let { Text(it, color = shed.danger, modifier = field) }
        Button(
            onClick = ::signIn,
            enabled = !busy && nexus.isNotBlank() && username.isNotBlank() && password.isNotEmpty() && totp.isNotBlank(),
            modifier = field,
        ) {
            if (busy) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text(stringResource(R.string.signin_button))
        }
    }
}
