package dev.shed.ui.workspaces

import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.Bench
import dev.shed.api.Call
import dev.shed.api.ClosedBench
import dev.shed.api.NewBench
import dev.shed.api.Policy
import dev.shed.api.Reply
import dev.shed.api.WorkspaceInfo
import dev.shed.api.WorkspaceState
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.common.Chip
import dev.shed.ui.common.Dot
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.LimitBars
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.VSpace
import dev.shed.ui.common.ago
import dev.shed.ui.common.aiCounts
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.common.tildePath
import dev.shed.ui.theme.LocalShed

/** Every workspace linked to the nexus, online or not, with its benches. */
@Composable
fun WorkspacesScreen(nav: NavController) {
    val socket = LocalSession.current.socket
    val workspaces by socket.workspaces.collectAsState()
    val states by socket.states.collectAsState()
    ShedScaffold(
        title = stringResource(R.string.app_name),
        onBack = null,
        actions = {
            IconButton(onClick = { nav.navigate(Routes.DEVICES) }) {
                Icon(Icons.Default.Devices, stringResource(R.string.devices_title))
            }
            IconButton(onClick = { nav.navigate(Routes.SETTINGS) }) {
                Icon(Icons.Default.Settings, stringResource(R.string.settings_title))
            }
        },
    ) { padding ->
        if (workspaces.isEmpty()) {
            EmptyNote(stringResource(R.string.workspaces_empty), Modifier.padding(padding))
        } else {
            LazyColumn(
                Modifier.fillMaxSize().padding(padding),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                items(workspaces, key = { it.id }) { ws -> WorkspaceCard(nav, ws, states[ws.id]) }
            }
        }
    }
}

@Composable
private fun WorkspaceCard(nav: NavController, ws: WorkspaceInfo, state: WorkspaceState?) {
    val shed = LocalShed.current
    val context = LocalContext.current
    val runner = rememberRunner()
    val session = LocalSession.current
    var showClosed by rememberSaveable(ws.id) { mutableStateOf(false) }
    var newBench by rememberSaveable { mutableStateOf(false) }
    val canChange = ws.online && ws.policy == Policy.Full
    Card(colors = CardDefaults.cardColors(containerColor = shed.surface)) {
        Column(Modifier.padding(vertical = 8.dp)) {
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Dot(if (ws.online) shed.good else shed.textDim, 10)
                HSpace(8)
                Column(Modifier.weight(1f)) {
                    Text(ws.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val seen = if (ws.online) {
                        stringResource(R.string.workspace_online)
                    } else {
                        stringResource(R.string.workspace_offline_since, ago(context, ws.seen.toLong()))
                    }
                    Text(
                        listOfNotNull(seen, ws.version?.takeIf { it.isNotEmpty() }?.let { "Shed $it" }).joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = shed.textDim,
                    )
                }
                if (ws.policy == Policy.View) {
                    Icon(Icons.Default.VisibilityOff, null, tint = shed.textDim)
                    HSpace(4)
                    Text(stringResource(R.string.workspace_view_only), style = MaterialTheme.typography.labelSmall, color = shed.textDim)
                }
                if (canChange) {
                    IconButton(onClick = { newBench = true }) { Icon(Icons.Default.Add, stringResource(R.string.bench_new)) }
                }
            }
            state?.limits?.let {
                LimitBars(it, Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
            }
            if (state == null) {
                Text(
                    stringResource(R.string.workspace_no_state),
                    color = shed.textDim,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(16.dp),
                )
            } else {
                if (state.benches.isEmpty()) {
                    Text(
                        stringResource(R.string.workspace_no_benches),
                        color = shed.textDim,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
                for (bench in state.benches) {
                    BenchRow(bench, state.home ?: "") { nav.navigate(Routes.bench(ws.id, bench.id)) }
                }
                if (state.closed.isNotEmpty()) {
                    TextButton(onClick = { showClosed = !showClosed }, modifier = Modifier.padding(horizontal = 4.dp)) {
                        Icon(if (showClosed) Icons.Default.ExpandLess else Icons.Default.ExpandMore, null)
                        HSpace(4)
                        Text(pluralStringResource(R.plurals.workspace_closed, state.closed.size, state.closed.size))
                    }
                    if (showClosed) {
                        for (closed in state.closed) {
                            ClosedRow(closed, state.home ?: "") { nav.navigate(Routes.closed(ws.id, closed.name)) }
                        }
                    }
                }
            }
        }
    }
    if (newBench) {
        TextDialog(
            title = stringResource(R.string.bench_new),
            initial = "",
            label = stringResource(R.string.bench_name_optional),
            confirm = stringResource(R.string.create),
            allowEmpty = true,
            onConfirm = { name ->
                runner.run {
                    val reply = session.socket.call(ws.id, Call.NewBench(NewBench(name.ifBlank { null }, null)))
                    if (reply is Reply.Bench) nav.navigate(Routes.bench(ws.id, reply.content))
                }
            },
            onDismiss = { newBench = false },
        )
    }
}

@Composable
private fun BenchRow(bench: Bench, home: String, onClick: () -> Unit) {
    val shed = LocalShed.current
    val color = benchColor(bench.color)
    val (working, waiting) = aiCounts(bench)
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .height(androidx.compose.foundation.layout.IntrinsicSize.Min),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(4.dp).fillMaxHeight().clip(RoundedCornerShape(2.dp)).background(color))
        HSpace(12)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    bench.name,
                    fontWeight = FontWeight.Medium,
                    color = shed.ink(color),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                bench.badge?.takeIf { it.isNotEmpty() }?.let {
                    HSpace(6)
                    Chip(it, color)
                }
            }
            Text(
                pluralStringResource(R.plurals.bench_tabs, bench.tabs.size, bench.tabs.size) + " · " + tildePath(bench.cwd, home),
                style = MaterialTheme.typography.bodySmall,
                color = shed.textDim,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (waiting > 0) {
            HSpace(6)
            Chip(pluralStringResource(R.plurals.ai_waiting_count, waiting, waiting), shed.warn, filled = true)
        }
        if (working > 0) {
            HSpace(6)
            Chip(pluralStringResource(R.plurals.ai_working_count, working, working), shed.accent)
        }
    }
}

@Composable
private fun ClosedRow(bench: ClosedBench, home: String, onClick: () -> Unit) {
    val shed = LocalShed.current
    val context = LocalContext.current
    val color = benchColor(bench.color)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Dot(color.copy(alpha = 0.6f))
        HSpace(12)
        Column(Modifier.weight(1f)) {
            Text(bench.name, color = shed.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                tildePath(bench.cwd, home) + " · " + ago(context, bench.saved.toLong()),
                style = MaterialTheme.typography.bodySmall,
                color = shed.textDim,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
    VSpace(0)
}
