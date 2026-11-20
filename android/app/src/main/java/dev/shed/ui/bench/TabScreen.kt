package dev.shed.ui.bench

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Upload
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.BenchRef
import dev.shed.api.Call
import dev.shed.api.ClosedName
import dev.shed.api.LoadBench
import dev.shed.api.RenameBench
import dev.shed.api.Reply
import dev.shed.api.TabKind
import dev.shed.net.Connection
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.ai.AiScreen
import dev.shed.ui.canChange
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.Loading
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.findTab
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.common.tildePath
import dev.shed.ui.project.ProjectScreen
import dev.shed.ui.terminal.TerminalScreen
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.workspaceState

/** A tab, shown by its kind. */
@Composable
fun TabScreen(nav: NavController, ws: String, tabId: String) {
    val state = workspaceState(ws)
    val found = state?.findTab(tabId)
    val connection by LocalSession.current.socket.connection.collectAsState()
    if (found == null && state == null && connection !is Connection.Online) {
        // Opened from a notification before the workspaces came: wait for them.
        ShedScaffold(title = "", onBack = { nav.popBackStack() }) { Loading(Modifier.padding(it)) }
        return
    }
    if (found == null) {
        ShedScaffold(title = stringResource(R.string.tab_gone), onBack = { nav.popBackStack() }) {
            EmptyNote(stringResource(R.string.tab_gone_text), Modifier.padding(it))
        }
        return
    }
    val (bench, tab) = found
    if (tab.private == true) {
        ShedScaffold(title = tab.title, onBack = { nav.popBackStack() }) {
            EmptyNote(stringResource(R.string.tab_private_note), Modifier.padding(it), Icons.Default.Lock)
        }
        return
    }
    when (val kind = tab.kind) {
        is TabKind.Ai -> AiScreen(nav, ws, tabId)
        is TabKind.Project -> ProjectScreen(nav, ws, bench, tab, kind.content)
        else -> TerminalScreen(nav, ws, bench, tab)
    }
}

/** A saved bench that is not running: its tabs as they will load, and loading it. */
@Composable
fun ClosedBenchScreen(nav: NavController, ws: String, name: String) {
    val state = workspaceState(ws)
    val bench = state?.closed?.firstOrNull { it.name == name }
    val session = LocalSession.current
    val runner = rememberRunner()
    val shed = LocalShed.current
    val canChange = canChange(ws)
    var menu by remember { mutableStateOf(false) }
    var renaming by rememberSaveable { mutableStateOf(false) }
    var deleting by rememberSaveable { mutableStateOf(false) }
    if (bench == null) {
        ShedScaffold(title = name, onBack = { nav.popBackStack() }) {
            EmptyNote(stringResource(R.string.bench_gone_text), Modifier.padding(it))
        }
        return
    }

    fun load(start: Boolean) = runner.run {
        val reply = session.socket.call(ws, Call.LoadBench(LoadBench(bench.name, start)))
        if (reply is Reply.Bench) {
            nav.popBackStack()
            nav.navigate(Routes.bench(ws, reply.content))
        }
    }

    ShedScaffold(
        title = bench.name,
        subtitle = stringResource(R.string.bench_closed_subtitle, tildePath(bench.cwd, state.home.orEmpty())),
        accent = benchColor(bench.color),
        onBack = { nav.popBackStack() },
        actions = {
            if (canChange) {
                Box {
                    IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.more)) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        MenuItem(Icons.Default.Edit, stringResource(R.string.bench_rename)) { menu = false; renaming = true }
                        MenuItem(Icons.Default.Delete, stringResource(R.string.bench_delete), shed.danger) { menu = false; deleting = true }
                    }
                }
            }
        },
        bottomBar = {
            if (canChange) {
                Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Button(onClick = { load(false) }, modifier = Modifier.weight(1f)) {
                        Icon(Icons.Default.Upload, null)
                        HSpace(8)
                        Text(stringResource(R.string.bench_load))
                    }
                    OutlinedButton(onClick = { load(true) }, modifier = Modifier.weight(1f)) {
                        Icon(Icons.Default.PlayArrow, null)
                        HSpace(8)
                        Text(stringResource(R.string.bench_load_start))
                    }
                }
            }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize()) {
            TabList(
                tabs = bench.tabs,
                active = null,
                canChange = false,
                padding = padding,
                onOpen = {},
                bench = BenchRef.Closed(bench.name),
                ws = ws,
            )
        }
    }
    if (renaming) {
        TextDialog(
            title = stringResource(R.string.bench_rename),
            initial = bench.name,
            confirm = stringResource(R.string.rename),
            onConfirm = {
                runner.run {
                    session.socket.call(ws, Call.RenameBench(RenameBench(BenchRef.Closed(bench.name), it)))
                    nav.popBackStack()
                }
            },
            onDismiss = { renaming = false },
        )
    }
    if (deleting) {
        ConfirmDialog(
            title = stringResource(R.string.bench_delete_title, bench.name),
            text = stringResource(R.string.bench_delete_text),
            confirm = stringResource(R.string.bench_delete),
            onConfirm = {
                runner.run {
                    session.socket.call(ws, Call.DeleteBench(ClosedName(bench.name)))
                    nav.popBackStack()
                }
            },
            onDismiss = { deleting = false },
        )
    }
}
