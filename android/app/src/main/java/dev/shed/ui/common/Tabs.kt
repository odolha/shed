package dev.shed.ui.common

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import dev.shed.api.AiActivity
import dev.shed.api.Bench
import dev.shed.api.Tab
import dev.shed.api.TabKind
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.parseColor

fun tabIcon(tab: Tab): ImageVector = when {
    tab.private == true -> Icons.Default.Lock
    else -> when (tab.kind) {
        is TabKind.Ai -> Icons.Default.AutoAwesome
        is TabKind.Project -> Icons.Default.Folder
        is TabKind.Remote -> Icons.Default.Cloud
        TabKind.Terminal -> Icons.Default.Terminal
    }
}

/** An AI tab's activity, else `null`. */
fun aiActivity(tab: Tab): AiActivity? = (tab.kind as? TabKind.Ai)?.content?.activity

/** A bench's colour; the accent if it does not read. */
@Composable
fun benchColor(color: String): Color = parseColor(color) ?: LocalShed.current.accent

/** How many AI tabs work and wait in a bench. */
fun aiCounts(bench: Bench): Pair<Int, Int> {
    var working = 0
    var waiting = 0
    for (tab in bench.tabs) {
        when (aiActivity(tab)) {
            AiActivity.Working -> working++
            AiActivity.Waiting -> waiting++
            else -> {}
        }
    }
    return working to waiting
}
