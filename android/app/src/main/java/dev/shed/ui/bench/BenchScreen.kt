package dev.shed.ui.bench

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.MenuBook
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Palette
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Save
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.AiActivity
import dev.shed.api.Bench
import dev.shed.api.BenchOnly
import dev.shed.api.BenchRef
import dev.shed.api.Call
import dev.shed.api.Divider
import dev.shed.api.Folder
import dev.shed.api.NewTab
import dev.shed.api.NewTabKind
import dev.shed.api.OptionalFolder
import dev.shed.api.RemoteTabArgs
import dev.shed.api.RenameBench
import dev.shed.api.RenameTab
import dev.shed.api.Reply
import dev.shed.api.SetBenchColor
import dev.shed.api.Tab
import dev.shed.api.TabKind
import dev.shed.api.TabOnly
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.canChange
import dev.shed.ui.common.Chip
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.aiActivity
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.bench
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.common.tabIcon
import dev.shed.ui.common.tildePath
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.parseColor
import dev.shed.ui.workspaceState
import kotlinx.coroutines.launch

/** Shed's named bench colours (`BenchColor`), with their bright stop. */
val BENCH_COLORS = listOf(
    "blue" to 0x5CB2FF, "violet" to 0xB48BFF, "pink" to 0xFF8AC8, "coral" to 0xFF9A5A,
    "amber" to 0xFFD15C, "green" to 0x7BE38E, "teal" to 0x4BE0C4, "slate" to 0xB3BAC7,
    "red" to 0xFF6B7A, "orange" to 0xFFB066, "lime" to 0xC6EF5A, "cyan" to 0x5FE3FF,
    "indigo" to 0x8C9BFF, "magenta" to 0xF28CFF, "brown" to 0xD9A47A, "olive" to 0xD2CC6E,
)

private enum class NewKind { Terminal, Project, Ai }

/** A running bench: its tabs, new tabs, its menu. */
@Composable
fun BenchScreen(nav: NavController, ws: String, benchId: String) {
    val state = workspaceState(ws)
    val bench = state?.bench(benchId)
    val home = state?.home.orEmpty()
    val session = LocalSession.current
    val runner = rememberRunner()
    val shed = LocalShed.current
    val canChange = canChange(ws)
    var menu by remember { mutableStateOf(false) }
    var renaming by rememberSaveable { mutableStateOf(false) }
    var coloring by rememberSaveable { mutableStateOf(false) }
    var closing by rememberSaveable { mutableStateOf(false) }
    var newMenu by remember { mutableStateOf(false) }
    var picking by rememberSaveable { mutableStateOf<String?>(null) }
    var remote by rememberSaveable { mutableStateOf(false) }

    if (bench == null) {
        ShedScaffold(title = stringResource(R.string.bench_gone), onBack = { nav.popBackStack() }) {
            EmptyNote(stringResource(R.string.bench_gone_text), Modifier.padding(it))
        }
        return
    }
    val color = benchColor(bench.color)

    fun newTab(kind: NewTabKind) = runner.run {
        val reply = session.socket.call(ws, Call.NewTab(NewTab(BenchRef.Running(bench.id), kind)))
        if (reply is Reply.Tab) nav.navigate(Routes.tab(ws, reply.content))
    }

    ShedScaffold(
        title = bench.name,
        subtitle = tildePath(bench.cwd, home),
        accent = color,
        onBack = { nav.popBackStack() },
        actions = {
            IconButton(onClick = { nav.navigate(Routes.logbook(ws, bench.tabs.firstOrNull()?.id)) }) {
                Icon(Icons.AutoMirrored.Filled.MenuBook, stringResource(R.string.logbook_title))
            }
            if (canChange) {
                Box {
                    IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.more)) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        MenuItem(Icons.Default.Edit, stringResource(R.string.bench_rename)) { menu = false; renaming = true }
                        MenuItem(Icons.Default.Palette, stringResource(R.string.bench_color)) { menu = false; coloring = true }
                        MenuItem(Icons.Default.PlayArrow, stringResource(R.string.bench_run_startup)) {
                            menu = false
                            runner.run { session.socket.call(ws, Call.RunStartup(BenchOnly(bench.id))) }
                        }
                        MenuItem(Icons.Default.Stop, stringResource(R.string.bench_run_teardown)) {
                            menu = false
                            runner.run { session.socket.call(ws, Call.RunTeardown(BenchOnly(bench.id))) }
                        }
                        MenuItem(Icons.Default.Save, stringResource(R.string.bench_save)) {
                            menu = false
                            runner.run { session.socket.call(ws, Call.SaveBench(BenchOnly(bench.id))) }
                        }
                        MenuItem(Icons.Default.Close, stringResource(R.string.bench_close), shed.danger) { menu = false; closing = true }
                    }
                }
            }
        },
        floatingActionButton = {
            if (canChange) {
                Box {
                    ExtendedFloatingActionButton(
                        onClick = { newMenu = true },
                        icon = { Icon(Icons.Default.Add, null) },
                        text = { Text(stringResource(R.string.tab_new)) },
                    )
                    DropdownMenu(expanded = newMenu, onDismissRequest = { newMenu = false }) {
                        MenuItem(Icons.Default.Terminal, stringResource(R.string.tab_new_terminal)) { newMenu = false; picking = NewKind.Terminal.name }
                        MenuItem(Icons.Default.Folder, stringResource(R.string.tab_new_project)) { newMenu = false; picking = NewKind.Project.name }
                        MenuItem(Icons.Default.AutoAwesome, stringResource(R.string.tab_new_ai)) { newMenu = false; picking = NewKind.Ai.name }
                        MenuItem(Icons.Default.Cloud, stringResource(R.string.tab_new_remote)) { newMenu = false; remote = true }
                    }
                }
            }
        },
    ) { padding ->
        TabList(
            tabs = bench.tabs,
            active = bench.activeTab,
            canChange = canChange,
            padding = padding,
            onOpen = { nav.navigate(Routes.tab(ws, it.id)) },
            bench = BenchRef.Running(bench.id),
            ws = ws,
        )
    }

    if (renaming) {
        TextDialog(
            title = stringResource(R.string.bench_rename),
            initial = bench.name,
            confirm = stringResource(R.string.rename),
            onConfirm = { runner.run { session.socket.call(ws, Call.RenameBench(RenameBench(BenchRef.Running(bench.id), it))) } },
            onDismiss = { renaming = false },
        )
    }
    if (coloring) {
        ColorDialog(
            onPick = { runner.run { session.socket.call(ws, Call.SetBenchColor(SetBenchColor(BenchRef.Running(bench.id), it))) } },
            onDismiss = { coloring = false },
        )
    }
    if (closing) {
        ConfirmDialog(
            title = stringResource(R.string.bench_close_title, bench.name),
            text = stringResource(R.string.bench_close_text),
            confirm = stringResource(R.string.bench_close),
            onConfirm = {
                runner.run {
                    session.socket.call(ws, Call.CloseBench(BenchOnly(bench.id)))
                    nav.popBackStack()
                }
            },
            onDismiss = { closing = false },
        )
    }
    picking?.let { kind ->
        FolderPicker(
            ws = ws,
            start = bench.cwd,
            home = home,
            title = stringResource(R.string.tab_pick_folder),
            onPick = { folder ->
                when (NewKind.valueOf(kind)) {
                    NewKind.Terminal -> newTab(NewTabKind.Terminal(OptionalFolder(folder)))
                    NewKind.Project -> newTab(NewTabKind.Project(Folder(folder)))
                    NewKind.Ai -> newTab(NewTabKind.Ai(Folder(folder)))
                }
            },
            onDismiss = { picking = null },
        )
    }
    if (remote) {
        TextDialog(
            title = stringResource(R.string.tab_new_remote),
            initial = "",
            label = stringResource(R.string.tab_remote_host),
            confirm = stringResource(R.string.open),
            onConfirm = { newTab(NewTabKind.Remote(RemoteTabArgs(it, null))) },
            onDismiss = { remote = false },
        )
    }
}

@Composable
fun MenuItem(icon: ImageVector, text: String, color: Color? = null, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(text, color = color ?: Color.Unspecified) },
        leadingIcon = { Icon(icon, null, tint = color ?: LocalShed.current.textDim) },
        onClick = onClick,
    )
}

@Composable
private fun ColorDialog(onPick: (String) -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.bench_color)) },
        text = {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                for ((name, rgb) in BENCH_COLORS) {
                    Box(
                        Modifier
                            .size(40.dp)
                            .clip(CircleShape)
                            .background(Color(0xFF000000 or rgb.toLong()))
                            .combinedClickable(onClickLabel = name, onClick = {
                                onDismiss()
                                onPick(name)
                            }),
                    )
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** A bench's tabs, in order, with their groups. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TabList(
    tabs: List<Tab>,
    active: String?,
    canChange: Boolean,
    padding: PaddingValues,
    onOpen: (Tab) -> Unit,
    bench: BenchRef,
    ws: String,
) {
    val shed = LocalShed.current
    val session = LocalSession.current
    val runner = rememberRunner()
    val snackbar = LocalSnackbar.current
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val privateText = stringResource(R.string.tab_private_note)
    var menuFor by remember { mutableStateOf<String?>(null) }
    var renaming by remember { mutableStateOf<Tab?>(null) }
    var closing by remember { mutableStateOf<Tab?>(null) }
    if (tabs.isEmpty()) {
        EmptyNote(stringResource(R.string.bench_no_tabs), Modifier.padding(padding))
        return
    }
    LazyColumn(Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(bottom = 96.dp)) {
        items(tabs, key = { it.id }) { tab ->
            when (tab.divider) {
                Divider.Space -> Spacer(Modifier.height(12.dp))
                Divider.Row -> HorizontalDivider(Modifier.padding(horizontal = 16.dp, vertical = 6.dp), color = shed.divider)
                else -> {}
            }
            Box {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .combinedClickable(
                            onClick = {
                                if (tab.private == true) {
                                    scope.launch { snackbar.showSnackbar(privateText) }
                                } else {
                                    onOpen(tab)
                                }
                            },
                            onLongClick = { if (canChange && bench is BenchRef.Running) menuFor = tab.id },
                        )
                        .background(if (tab.id == active) shed.surface else Color.Transparent)
                        .padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val tinted = tab.tags.orEmpty().firstOrNull { it.tint == true }
                    Icon(tabIcon(tab), null, tint = tinted?.let { parseColor(it.color) }?.let { shed.ink(it) } ?: shed.textDim)
                    HSpace(14)
                    Column(Modifier.weight(1f)) {
                        Text(
                            tab.title,
                            fontWeight = if (tab.id == active) FontWeight.Medium else FontWeight.Normal,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        val tags = tab.tags.orEmpty()
                        if (tags.isNotEmpty()) {
                            Row(horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.padding(top = 2.dp)) {
                                for (tag in tags) Chip(tag.label, parseColor(tag.color) ?: shed.accent)
                            }
                        }
                        val panes = tab.panes.orEmpty()
                        val pane = panes.firstOrNull { it.id == tab.activePane } ?: panes.firstOrNull()
                        pane?.tool?.let {
                            Text(it, style = MaterialTheme.typography.bodySmall, color = shed.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    when (aiActivity(tab)) {
                        AiActivity.Waiting -> Chip(stringResource(R.string.ai_waiting), shed.warn, filled = true)
                        AiActivity.Working -> Chip(stringResource(R.string.ai_working), shed.accent)
                        else -> {}
                    }
                    if ((tab.panes?.size ?: 0) > 1) {
                        HSpace(6)
                        Chip(pluralStringResource(R.plurals.tab_panes, tab.panes!!.size, tab.panes!!.size), shed.textDim)
                    }
                    if (tab.id == active) {
                        HSpace(8)
                        Box(Modifier.width(3.dp).height(20.dp).background(shed.accent))
                    }
                }
                DropdownMenu(expanded = menuFor == tab.id, onDismissRequest = { menuFor = null }) {
                    MenuItem(Icons.Default.Edit, stringResource(R.string.tab_rename)) { menuFor = null; renaming = tab }
                    if (tab.kind is TabKind.Remote) {
                        MenuItem(Icons.Default.Refresh, stringResource(R.string.tab_reconnect)) {
                            menuFor = null
                            runner.run { session.socket.call(ws, Call.ReconnectRemote(TabOnly(tab.id))) }
                        }
                    }
                    MenuItem(Icons.Default.Close, stringResource(R.string.tab_close), shed.danger) { menuFor = null; closing = tab }
                }
            }
        }
    }
    renaming?.let { tab ->
        TextDialog(
            title = stringResource(R.string.tab_rename),
            initial = tab.name ?: tab.title,
            confirm = stringResource(R.string.rename),
            allowEmpty = true,
            onConfirm = { name ->
                runner.run { session.socket.call(ws, Call.RenameTab(RenameTab(bench, tab.id, name.ifBlank { null }))) }
            },
            onDismiss = { renaming = null },
        )
    }
    closing?.let { tab ->
        ConfirmDialog(
            title = stringResource(R.string.tab_close_title, tab.title),
            text = stringResource(R.string.tab_close_text),
            confirm = stringResource(R.string.tab_close),
            onConfirm = { runner.run { session.socket.call(ws, Call.CloseTab(TabOnly(tab.id))) } },
            onDismiss = { closing = null },
        )
    }
}

