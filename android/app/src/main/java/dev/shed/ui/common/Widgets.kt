package dev.shed.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shed.R
import dev.shed.api.LimitWindow
import dev.shed.api.Limits
import dev.shed.ui.theme.LocalShed
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/** Where screens say what happened (errors mostly). */
val LocalSnackbar = staticCompositionLocalOf { SnackbarHostState() }

/** Runs work that may fail, showing why in the snackbar. */
class Runner(private val scope: CoroutineScope, private val show: (Throwable) -> Unit) {
    fun run(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                show(e)
            }
        }
    }
}

@Composable
fun rememberRunner(): Runner {
    val scope = rememberCoroutineScope()
    val snackbar = LocalSnackbar.current
    val context = LocalContext.current
    return remember(scope, snackbar) {
        Runner(scope) { e -> scope.launch { snackbar.showSnackbar(errorText(context, e)) } }
    }
}

/** Asks before something that cannot be taken back. */
@Composable
fun ConfirmDialog(
    title: String,
    text: String,
    confirm: String,
    danger: Boolean = true,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    val shed = LocalShed.current
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(text) },
        confirmButton = {
            TextButton(
                onClick = {
                    onDismiss()
                    onConfirm()
                },
                colors = if (danger) ButtonDefaults.textButtonColors(contentColor = shed.danger) else ButtonDefaults.textButtonColors(),
            ) { Text(confirm) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** Asks for a line of text: a name, a message. */
@Composable
fun TextDialog(
    title: String,
    initial: String,
    confirm: String,
    label: String = "",
    allowEmpty: Boolean = false,
    onConfirm: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var value by rememberSaveable { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = value,
                onValueChange = { value = it },
                label = if (label.isNotEmpty()) ({ Text(label) }) else null,
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(
                enabled = allowEmpty || value.isNotBlank(),
                onClick = {
                    onDismiss()
                    onConfirm(value.trim())
                },
            ) { Text(confirm) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** Asks for a fresh code from the authenticator (for revoking, pairing). */
@Composable
fun TotpDialog(
    title: String,
    text: String,
    confirm: String,
    danger: Boolean = false,
    nameLabel: String? = null,
    initialName: String = "",
    onConfirm: (code: String, name: String) -> Unit,
    onDismiss: () -> Unit,
) {
    val shed = LocalShed.current
    var code by rememberSaveable { mutableStateOf("") }
    var name by rememberSaveable { mutableStateOf(initialName) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (text.isNotEmpty()) Text(text)
                if (nameLabel != null) {
                    OutlinedTextField(
                        value = name,
                        onValueChange = { name = it },
                        label = { Text(nameLabel) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                OutlinedTextField(
                    value = code,
                    onValueChange = { code = it.filter { c -> c.isDigit() }.take(8) },
                    label = { Text(stringResource(R.string.totp_code)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            TextButton(
                enabled = code.length >= 6 && (nameLabel == null || name.isNotBlank()),
                onClick = {
                    onDismiss()
                    onConfirm(code, name.trim())
                },
                colors = if (danger) ButtonDefaults.textButtonColors(contentColor = shed.danger) else ButtonDefaults.textButtonColors(),
            ) { Text(confirm) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
fun Loading(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
}

/** A quiet message in the middle of an empty screen. */
@Composable
fun EmptyNote(text: String, modifier: Modifier = Modifier, icon: ImageVector? = null) {
    val shed = LocalShed.current
    Column(
        modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (icon != null) {
            Icon(icon, null, tint = shed.textDim, modifier = Modifier.size(40.dp))
            Spacer(Modifier.height(12.dp))
        }
        Text(text, color = shed.textDim, style = MaterialTheme.typography.bodyMedium)
    }
}

/** A small rounded label. */
@Composable
fun Chip(text: String, color: Color, modifier: Modifier = Modifier, filled: Boolean = false) {
    val shed = LocalShed.current
    Text(
        text,
        color = if (filled) shed.background else shed.ink(color),
        fontSize = 11.sp,
        maxLines = 1,
        modifier = modifier
            .clip(RoundedCornerShape(6.dp))
            .background(if (filled) color else color.copy(alpha = 0.16f))
            .padding(horizontal = 6.dp, vertical = 1.dp),
    )
}

@Composable
fun Dot(color: Color, size: Int = 8) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(color))
}

/** The account's usage limits, as two small bars (green, amber from 70 %, red from 90 %). */
@Composable
fun LimitBars(limits: Limits?, modifier: Modifier = Modifier) {
    if (limits == null || (limits.fiveHour == null && limits.week == null)) return
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        limits.fiveHour?.let { LimitBar(stringResource(R.string.limit_five_hour), it, Modifier.weight(1f)) }
        limits.week?.let { LimitBar(stringResource(R.string.limit_week), it, Modifier.weight(1f)) }
    }
}

@Composable
private fun LimitBar(name: String, window: LimitWindow, modifier: Modifier) {
    val shed = LocalShed.current
    val used = window.used.coerceIn(0f, 1f)
    val color = when {
        used >= 0.9f -> shed.danger
        used >= 0.7f -> shed.warn
        else -> shed.good
    }
    val left = window.resets.toLong() - nowSeconds()
    Column(modifier) {
        Row(Modifier.fillMaxWidth()) {
            Text(name, fontSize = 11.sp, color = shed.textDim, modifier = Modifier.weight(1f))
            Text("${(used * 100).toInt()} %", fontSize = 11.sp, color = shed.text)
        }
        Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(shed.divider)) {
            Box(Modifier.fillMaxWidth(used).height(4.dp).background(color))
        }
        if (left > 0) {
            Text(stringResource(R.string.limit_left, span(left)), fontSize = 10.sp, color = shed.textDim)
        }
    }
}

/** A row label with the space after it. */
@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text.uppercase(),
        style = MaterialTheme.typography.labelSmall,
        color = LocalShed.current.textDim,
        modifier = modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp),
    )
}

@Composable
fun HSpace(width: Int) = Spacer(Modifier.width(width.dp))

@Composable
fun VSpace(height: Int) = Spacer(Modifier.height(height.dp))
