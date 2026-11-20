package dev.shed.ui.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.material.icons.filled.Computer
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Language
import androidx.compose.material.icons.filled.PhoneAndroid
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.Device
import dev.shed.api.DeviceKind
import dev.shed.api.PairApprove
import dev.shed.api.PairReject
import dev.shed.api.PendingPairing
import dev.shed.api.RenameDevice
import dev.shed.api.Revoke
import dev.shed.net.Empty
import dev.shed.ui.LocalSession
import dev.shed.ui.common.Chip
import dev.shed.ui.common.Dot
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.Loading
import dev.shed.ui.common.SectionTitle
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.TotpDialog
import dev.shed.ui.common.ago
import dev.shed.ui.common.errorText
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.theme.LocalShed

/** The devices signed in or paired, workspaces waiting to be paired. */
@Composable
fun DevicesScreen(nav: NavController) {
    val http = LocalSession.current.http
    val context = LocalContext.current
    val shed = LocalShed.current
    val runner = rememberRunner()
    var refresh by remember { mutableIntStateOf(0) }
    var devices by remember { mutableStateOf<List<Device>?>(null) }
    var pending by remember { mutableStateOf<List<PendingPairing>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var revoking by remember { mutableStateOf<Device?>(null) }
    var renaming by remember { mutableStateOf<Device?>(null) }
    var approving by remember { mutableStateOf<PendingPairing?>(null) }

    LaunchedEffect(refresh) {
        try {
            devices = http.getList("/devices", Device.serializer())
            pending = http.getList("/pair/pending", PendingPairing.serializer())
            error = null
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
            if (devices == null) devices = emptyList()
        }
    }

    ShedScaffold(
        title = stringResource(R.string.devices_title),
        onBack = { nav.popBackStack() },
        actions = { IconButton(onClick = { refresh++ }) { Icon(Icons.Default.Refresh, stringResource(R.string.refresh)) } },
    ) { padding ->
        val list = devices
        if (list == null) {
            Loading(Modifier.padding(padding))
            return@ShedScaffold
        }
        LazyColumn(Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(bottom = 24.dp)) {
            error?.let { item { Text(it, color = shed.danger, modifier = Modifier.padding(16.dp)) } }
            item {
                TextButton(onClick = { approving = PendingPairing("", "", "", "", null, 0u) }, modifier = Modifier.padding(horizontal = 8.dp)) {
                    Icon(Icons.Default.Add, null)
                    HSpace(6)
                    Text(stringResource(R.string.pair_workspace))
                }
            }
            if (pending.isNotEmpty()) {
                item { SectionTitle(stringResource(R.string.pair_pending)) }
                items(pending.size) { index ->
                    val p = pending[index]
                    Card(
                        colors = CardDefaults.cardColors(containerColor = shed.surface),
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                    ) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(stringResource(R.string.pair_asks, p.name, p.version), fontWeight = FontWeight.Medium)
                            Text(
                                listOfNotNull(p.address.takeIf { it.isNotEmpty() }, p.country).joinToString(" · "),
                                color = shed.textDim,
                                style = MaterialTheme.typography.bodySmall,
                            )
                            TextButton(onClick = { approving = p }) { Text(stringResource(R.string.pair_approve_ellipsis)) }
                        }
                    }
                }
            }
            for ((kind, title) in listOf(
                DeviceKind.Workspace to R.string.devices_workspaces,
                DeviceKind.Phone to R.string.devices_phones,
                DeviceKind.Browser to R.string.devices_browsers,
            )) {
                val ofKind = list.filter { it.kind == kind }
                if (ofKind.isEmpty()) continue
                item { SectionTitle(stringResource(title)) }
                items(ofKind, key = { it.id }) { device ->
                    Row(
                        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(
                            when (device.kind) {
                                DeviceKind.Workspace -> Icons.Default.Computer
                                DeviceKind.Phone -> Icons.Default.PhoneAndroid
                                DeviceKind.Browser -> Icons.Default.Language
                            },
                            null,
                            tint = shed.textDim,
                        )
                        HSpace(12)
                        Column(Modifier.weight(1f)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(device.name, maxLines = 1)
                                if (device.online == true) {
                                    HSpace(6)
                                    Dot(shed.good)
                                }
                                if (device.current == true) {
                                    HSpace(6)
                                    Chip(stringResource(R.string.devices_this), shed.accent)
                                }
                            }
                            Text(
                                listOfNotNull(
                                    stringResource(R.string.devices_seen, ago(context, device.seen.toLong())),
                                    device.address?.takeIf { it.isNotEmpty() },
                                    device.country,
                                ).joinToString(" · "),
                                color = shed.textDim,
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        IconButton(onClick = { renaming = device }) { Icon(Icons.Default.Edit, stringResource(R.string.rename)) }
                        if (device.current != true) {
                            TextButton(
                                onClick = { revoking = device },
                                colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                            ) { Text(stringResource(R.string.devices_revoke)) }
                        }
                    }
                }
            }
        }
    }

    revoking?.let { device ->
        TotpDialog(
            title = stringResource(R.string.devices_revoke_title, device.name),
            text = stringResource(R.string.devices_revoke_text),
            confirm = stringResource(R.string.devices_revoke),
            danger = true,
            onConfirm = { code, _ ->
                runner.run {
                    http.post("/devices/revoke", Revoke(device.id, code), Revoke.serializer(), Empty.serializer())
                    refresh++
                }
            },
            onDismiss = { revoking = null },
        )
    }
    renaming?.let { device ->
        TextDialog(
            title = stringResource(R.string.devices_rename),
            initial = device.name,
            confirm = stringResource(R.string.rename),
            onConfirm = { name ->
                runner.run {
                    http.post("/devices/rename", RenameDevice(device.id, name), RenameDevice.serializer(), Empty.serializer())
                    refresh++
                }
            },
            onDismiss = { renaming = null },
        )
    }
    approving?.let { p ->
        PairDialog(
            initialName = p.name,
            onApprove = { code, name, totp ->
                runner.run {
                    http.post("/pair/approve", PairApprove(code, name, totp), PairApprove.serializer(), Empty.serializer())
                    refresh++
                }
            },
            onReject = { code ->
                runner.run {
                    http.post("/pair/reject", PairReject(code), PairReject.serializer(), Empty.serializer())
                    refresh++
                }
            },
            onDismiss = { approving = null },
        )
    }
}

/** A pairing code as typed (`k7qf 2m9d`, `K7QF2M9D`) in the form the workspace shows: `K7QF-2M9D`. */
fun normalizePairCode(typed: String): String {
    val bare = typed.uppercase().filter { it.isLetterOrDigit() }
    return if (bare.length == 8) bare.take(4) + "-" + bare.drop(4) else typed.trim().uppercase()
}

/**
 * Approving a workspace's pairing: the code that `shed nexus join` printed on
 * it, typed here (never taken from the list), its name, and a fresh TOTP code.
 */
@Composable
private fun PairDialog(
    initialName: String,
    onApprove: (code: String, name: String, totp: String) -> Unit,
    onReject: (code: String) -> Unit,
    onDismiss: () -> Unit,
) {
    val shed = LocalShed.current
    var code by rememberSaveable { mutableStateOf("") }
    var name by rememberSaveable { mutableStateOf(initialName) }
    var totp by rememberSaveable { mutableStateOf("") }
    val codeOk = code.count { it.isLetterOrDigit() } >= 6
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.pair_approve_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.pair_approve_text))
                OutlinedTextField(
                    value = code,
                    onValueChange = { code = it.take(16) },
                    label = { Text(stringResource(R.string.pair_code)) },
                    placeholder = { Text("K7QF-2M9D") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text(stringResource(R.string.pair_name)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = totp,
                    onValueChange = { totp = it.filter { c -> c.isDigit() }.take(8) },
                    label = { Text(stringResource(R.string.totp_code)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            TextButton(
                enabled = codeOk && name.isNotBlank() && totp.length >= 6,
                onClick = {
                    onDismiss()
                    onApprove(normalizePairCode(code), name.trim(), totp)
                },
            ) { Text(stringResource(R.string.pair_approve)) }
        },
        dismissButton = {
            Row {
                TextButton(
                    enabled = codeOk,
                    onClick = {
                        onDismiss()
                        onReject(normalizePairCode(code))
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                ) { Text(stringResource(R.string.pair_reject)) }
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
            }
        },
    )
}
