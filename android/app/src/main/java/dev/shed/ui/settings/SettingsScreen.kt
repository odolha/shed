package dev.shed.ui.settings

import android.Manifest
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Logout
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.MainActivity
import dev.shed.R
import dev.shed.api.NotificationKind
import dev.shed.api.PushInfo
import dev.shed.api.PushPrefs
import dev.shed.data.ThemeChoice
import dev.shed.net.Empty
import dev.shed.push.Push
import dev.shed.push.PushMode
import dev.shed.push.leaveUnifiedPush
import dev.shed.push.pushMode
import dev.shed.shed
import dev.shed.ui.LocalSession
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.SectionTitle
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.errorText
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.logbook.copy
import dev.shed.ui.theme.LocalShed
import kotlinx.coroutines.launch

@Composable
fun SettingsScreen(nav: NavController) {
    val context = LocalContext.current
    val activity = context as? MainActivity
    val app = context.shed
    val session = LocalSession.current
    val shed = LocalShed.current
    val runner = rememberRunner()
    val scope = rememberCoroutineScope()
    val theme by app.theme.collectAsState()
    var lock by remember { mutableStateOf(app.settings.biometricLock) }
    var pushOn by remember { mutableStateOf(app.settings.pushEnabled) }
    var info by remember { mutableStateOf<PushInfo?>(null) }
    var pushError by remember { mutableStateOf<String?>(null) }
    var signingOut by remember { mutableStateOf(false) }
    val distributors = remember { Push.distributors(context) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    val snackbar = LocalSnackbar.current

    val pushChanged by Push.changed.collectAsState()
    LaunchedEffect(pushChanged) {
        try {
            val got = session.http.get("/push", PushInfo.serializer())
            info = got
            // The nexus knows whether it pushes here (older ones do not say).
            got.subscribed?.let { pushOn = it }
            pushError = null
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            pushError = errorText(context, e)
        }
    }
    val mode = info?.let(::pushMode)
    // The nexus reaches phones through its ntfy topic: no UnifiedPush here too.
    LaunchedEffect(mode) {
        if (mode != null && leaveUnifiedPush(mode, Push.registered(context))) {
            pushOn = false
            Push.disable(context)
        }
    }

    ShedScaffold(title = stringResource(R.string.settings_title), onBack = { nav.popBackStack() }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState())) {
            SectionTitle(stringResource(R.string.settings_account))
            Text(
                stringResource(R.string.settings_signed_in, session.account.username, session.account.nexus),
                modifier = Modifier.padding(horizontal = 16.dp),
            )

            SectionTitle(stringResource(R.string.settings_notifications))
            val prefs = info?.prefs?.kinds.orEmpty()
            val kindSwitches = @Composable {
                for ((kind, label) in listOf(
                    NotificationKind.AiWaiting to R.string.channel_ai_waiting,
                    NotificationKind.AiDone to R.string.channel_ai_done,
                    NotificationKind.CommandDone to R.string.channel_command_done,
                    NotificationKind.WorkspaceOffline to R.string.channel_workspace_offline,
                )) {
                    SwitchRow(stringResource(label), kind in prefs, indent = true) { on ->
                        val kinds = if (on) prefs + kind else prefs - kind
                        runner.run {
                            session.http.post("/push/prefs", PushPrefs(kinds), PushPrefs.serializer(), Empty.serializer())
                            info = info?.copy(prefs = PushPrefs(kinds))
                        }
                    }
                }
            }
            val vapid = info?.vapidKey
            when {
                mode is PushMode.Ntfy -> {
                    Text(stringResource(R.string.push_ntfy_topic, mode.topic), modifier = Modifier.padding(horizontal = 16.dp))
                    val copied = stringResource(R.string.copied)
                    OutlinedButton(
                        onClick = {
                            copy(context, mode.topic)
                            scope.launch { snackbar.showSnackbar(copied) }
                        },
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                    ) {
                        Icon(Icons.Default.ContentCopy, null)
                        HSpace(8)
                        Text(stringResource(R.string.copy))
                    }
                    Text(
                        stringResource(R.string.push_ntfy_kinds),
                        color = shed.textDim,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(horizontal = 16.dp),
                    )
                    kindSwitches()
                }
                distributors.isEmpty() -> Text(
                    stringResource(R.string.push_no_distributor),
                    color = shed.textDim,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
                info != null && vapid == null -> Text(
                    stringResource(R.string.push_off_at_nexus),
                    color = shed.textDim,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
                else -> {
                    SwitchRow(stringResource(R.string.push_enable), pushOn) { on ->
                        if (on) {
                            if (Build.VERSION.SDK_INT >= 33) permission.launch(Manifest.permission.POST_NOTIFICATIONS)
                            if (activity != null && vapid != null) {
                                Push.enable(activity, vapid) { ok -> pushOn = ok }
                            }
                        } else {
                            pushOn = false
                            Push.disable(context) { ok -> if (!ok) pushError = context.getString(R.string.push_unsubscribe_failed) }
                        }
                    }
                    if (!pushOn && info?.subscribed == false && app.settings.pushEnabled) {
                        Text(
                            stringResource(R.string.push_not_subscribed),
                            color = shed.textDim,
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(horizontal = 16.dp),
                        )
                    }
                    if (pushOn) kindSwitches()
                }
            }
            Text(
                stringResource(R.string.push_live),
                color = shed.textDim,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
            )
            pushError?.let { Text(it, color = shed.danger, modifier = Modifier.padding(horizontal = 16.dp)) }

            SectionTitle(stringResource(R.string.settings_security))
            if (activity?.canUnlock() == true) {
                SwitchRow(stringResource(R.string.settings_biometric), lock) { on ->
                    activity.authenticate { ok ->
                        if (ok) {
                            app.settings.biometricLock = on
                            lock = on
                        }
                    }
                }
            } else {
                Text(stringResource(R.string.settings_biometric_none), color = shed.textDim, modifier = Modifier.padding(horizontal = 16.dp))
            }

            SectionTitle(stringResource(R.string.settings_theme))
            Row(Modifier.padding(horizontal = 16.dp)) {
                for ((choice, label) in listOf(
                    ThemeChoice.Dark to R.string.theme_dark,
                    ThemeChoice.Light to R.string.theme_light,
                    ThemeChoice.System to R.string.theme_system,
                )) {
                    FilterChip(theme == choice, { app.setTheme(choice) }, { Text(stringResource(label)) })
                    HSpace(8)
                }
            }

            SectionTitle(stringResource(R.string.settings_about))
            Text(
                stringResource(R.string.settings_version, dev.shed.BuildConfig.VERSION_NAME, session.socket.nexusVersion.ifEmpty { "?" }),
                color = shed.textDim,
                modifier = Modifier.padding(horizontal = 16.dp),
            )

            OutlinedButton(
                onClick = { signingOut = true },
                colors = ButtonDefaults.outlinedButtonColors(contentColor = shed.danger),
                modifier = Modifier.fillMaxWidth().padding(16.dp),
            ) {
                Icon(Icons.AutoMirrored.Filled.Logout, null)
                HSpace(8)
                Text(stringResource(R.string.sign_out))
            }
        }
    }
    if (signingOut) {
        ConfirmDialog(
            title = stringResource(R.string.sign_out_title),
            text = stringResource(R.string.sign_out_text),
            confirm = stringResource(R.string.sign_out),
            onConfirm = { scope.launch { app.signOut() } },
            onDismiss = { signingOut = false },
        )
    }
}

@Composable
private fun SwitchRow(label: String, checked: Boolean, indent: Boolean = false, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(start = if (indent) 32.dp else 16.dp, end = 16.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        Switch(checked, onChange)
    }
}
