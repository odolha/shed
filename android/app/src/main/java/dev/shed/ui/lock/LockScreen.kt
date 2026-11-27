package dev.shed.ui.lock

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material.icons.filled.LockOpen
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.shed.R
import dev.shed.ui.common.HSpace
import dev.shed.ui.theme.LocalShed

/**
 * The app locked: nothing of the workspaces, only the way to unlock (the
 * prompt comes up by itself; Unlock brings it back). Opaque, and it takes
 * every touch, so nothing under it can be used.
 */
@Composable
fun LockScreen(onUnlock: () -> Unit) {
    val shed = LocalShed.current
    Surface(Modifier.fillMaxSize(), color = shed.background) {
        Column(
            Modifier.fillMaxSize().systemBarsPadding().padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(Icons.Default.Fingerprint, null, Modifier.size(72.dp), tint = shed.accent)
            Text(stringResource(R.string.lock_title), style = MaterialTheme.typography.headlineSmall)
            Text(
                stringResource(R.string.lock_text),
                color = shed.textDim,
                textAlign = TextAlign.Center,
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.widthIn(max = 360.dp),
            )
            Button(onClick = onUnlock) {
                Icon(Icons.Default.LockOpen, null)
                HSpace(8)
                Text(stringResource(R.string.lock_unlock))
            }
        }
    }
}
