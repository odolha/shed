package dev.shed.ui.logbook

import androidx.annotation.StringRes
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.FolderCopy
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import dev.shed.api.BenchRef
import dev.shed.api.CommandTab
import dev.shed.api.NewTab
import dev.shed.api.NewTabKind
import dev.shed.ui.Routes
import dev.shed.ui.canChange
import dev.shed.ui.common.ChoiceMenu
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.ListScrollbar
import dev.shed.ui.common.MenuChoice
import dev.shed.ui.common.findTab
import dev.shed.ui.common.rememberRunner
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.consumeWindowInsets
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.Call
import dev.shed.api.LogEntry
import dev.shed.api.LogScope
import dev.shed.api.LogSearch
import dev.shed.api.Reply
import dev.shed.ui.LocalSession
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.Loading
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.ago
import dev.shed.ui.common.errorText
import dev.shed.ui.common.span
import dev.shed.ui.common.tildePath
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono
import dev.shed.ui.workspaceState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** What a long press on a logbook entry offers. */
enum class LogMenu(@StringRes override val label: Int) : MenuChoice {
    CopyCommand(R.string.logbook_copy_command),
    CopyFolder(R.string.logbook_copy_folder),
    Run(R.string.logbook_run),
    ;

    override val icon: ImageVector get() = when (this) {
        CopyCommand -> Icons.Default.ContentCopy
        CopyFolder -> Icons.Default.FolderCopy
        Run -> Icons.Default.PlayArrow
    }
}

/** A logbook entry's menu: running it again only where you may change things, with a bench to run it in. */
fun logMenu(canRun: Boolean): List<LogMenu> =
    if (canRun) LogMenu.entries else listOf(LogMenu.CopyCommand, LogMenu.CopyFolder)

/**
 * The running bench to run an entry again in: its own, if it still runs,
 * else the one the logbook was opened from; `null`: none.
 */
fun runBench(entry: LogEntry, from: String?, running: List<String>): String? =
    entry.bench?.takeIf { it in running } ?: from?.takeIf { it in running }

/** Running an entry again: a new terminal tab in its folder that runs it. */
fun runAgain(entry: LogEntry, bench: String): Call =
    Call.NewTab(NewTab(BenchRef.Running(bench), NewTabKind.Command(CommandTab(entry.cwd, entry.command))))

/** The workspace's command history: this tab's, this bench's, or all. */
@Composable
fun LogbookScreen(nav: NavController, ws: String, tab: String?) {
    val session = LocalSession.current
    val context = LocalContext.current
    val shed = LocalShed.current
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    val home = workspaceState(ws)?.home.orEmpty()
    var scope2 by rememberSaveable { mutableStateOf(if (tab != null) LogScope.Tab else LogScope.All) }
    var query by rememberSaveable { mutableStateOf("") }
    var entries by remember { mutableStateOf<List<LogEntry>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val copied = stringResource(R.string.copied)
    val state = workspaceState(ws)
    val fromBench = tab?.let { state?.findTab(it)?.first?.id }
    val running = state?.benches.orEmpty().map { it.id }
    val canChange = canChange(ws)
    val runner = rememberRunner()
    var menuFor by remember { mutableStateOf<LogEntry?>(null) }
    var running1 by remember { mutableStateOf<Pair<LogEntry, String>?>(null) }
    fun copyText(text: String) {
        copy(context, text)
        scope.launch { snackbar.showSnackbar(copied) }
    }
    LaunchedEffect(scope2, query) {
        delay(250)
        error = null
        try {
            val reply = session.socket.call(ws, Call.LogSearch(LogSearch(scope2, tab, query, 300u)))
            entries = (reply as? Reply.Log)?.content.orEmpty()
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
            entries = emptyList()
        }
    }
    ShedScaffold(title = stringResource(R.string.logbook_title), onBack = { nav.popBackStack() }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                leadingIcon = { Icon(Icons.Default.Search, null) },
                placeholder = { Text(stringResource(R.string.logbook_search)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
            )
            Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (tab != null) {
                    FilterChip(scope2 == LogScope.Tab, { scope2 = LogScope.Tab }, { Text(stringResource(R.string.logbook_scope_tab)) })
                    FilterChip(scope2 == LogScope.Bench, { scope2 = LogScope.Bench }, { Text(stringResource(R.string.logbook_scope_bench)) })
                }
                FilterChip(scope2 == LogScope.All, { scope2 = LogScope.All }, { Text(stringResource(R.string.logbook_scope_all)) })
            }
            error?.let { Text(it, color = shed.danger, modifier = Modifier.padding(12.dp)) }
            val list = entries
            when {
                list == null -> Loading()
                list.isEmpty() -> EmptyNote(stringResource(R.string.logbook_empty))
                else -> Box(Modifier.fillMaxSize()) {
                  val listState = rememberLazyListState()
                  LazyColumn(state = listState, contentPadding = PaddingValues(vertical = 8.dp)) {
                    items(list) { entry ->
                        val bench = if (canChange) runBench(entry, fromBench, running) else null
                        Box {
                        Column(
                            Modifier
                                .fillMaxWidth()
                                .combinedClickable(
                                    onClickLabel = stringResource(R.string.logbook_copy_command),
                                    onLongClickLabel = stringResource(R.string.more),
                                    onLongClick = {
                                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                        menuFor = entry
                                    },
                                ) { copyText(entry.command) }
                                .padding(horizontal = 16.dp, vertical = 8.dp),
                        ) {
                            Text(entry.command, fontFamily = Mono, maxLines = 3, overflow = TextOverflow.Ellipsis)
                            val exit = entry.exit
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                if (exit != null && exit != 0) {
                                    Text(stringResource(R.string.logbook_exit, exit), color = shed.danger, style = MaterialTheme.typography.bodySmall)
                                }
                                Text(
                                    listOf(
                                        tildePath(entry.cwd, home),
                                        ago(context, entry.started.toLong()),
                                        span(entry.duration.toLong()),
                                    ).joinToString(" · "),
                                    color = shed.textDim,
                                    style = MaterialTheme.typography.bodySmall,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                        }
                        ChoiceMenu(menuFor == entry, logMenu(bench != null), onDismiss = { menuFor = null }) {
                            when (it) {
                                LogMenu.CopyCommand -> copyText(entry.command)
                                LogMenu.CopyFolder -> copyText(entry.cwd)
                                LogMenu.Run -> bench?.let { b -> running1 = entry to b }
                            }
                        }
                        }
                    }
                  }
                  ListScrollbar(listState, Modifier.align(Alignment.TopEnd))
                }
            }
        }
    }
    running1?.let { (entry, bench) ->
        RunDialog(entry, home, onRun = {
            runner.run {
                val reply = session.socket.call(ws, runAgain(entry, bench))
                if (reply is Reply.Tab) nav.navigate(Routes.tab(ws, reply.content))
            }
        }) { running1 = null }
    }
}

@Composable
private fun RunDialog(entry: LogEntry, home: String, onRun: () -> Unit, onDismiss: () -> Unit) {
    ConfirmDialog(
        title = stringResource(R.string.logbook_run_title),
        text = stringResource(R.string.logbook_run_text, entry.command, tildePath(entry.cwd, home)),
        confirm = stringResource(R.string.logbook_run_confirm),
        danger = false,
        onConfirm = onRun,
        onDismiss = onDismiss,
    )
}

fun copy(context: Context, text: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText("Shed", text))
}
