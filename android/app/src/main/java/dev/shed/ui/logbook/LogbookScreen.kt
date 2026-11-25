package dev.shed.ui.logbook

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

/** The workspace's command history: this tab's, this bench's, or all. */
@Composable
fun LogbookScreen(nav: NavController, ws: String, tab: String?) {
    val session = LocalSession.current
    val context = LocalContext.current
    val shed = LocalShed.current
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    val home = workspaceState(ws)?.home.orEmpty()
    var scope2 by rememberSaveable { mutableStateOf(if (tab != null) LogScope.Tab else LogScope.All) }
    var query by rememberSaveable { mutableStateOf("") }
    var entries by remember { mutableStateOf<List<LogEntry>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val copied = stringResource(R.string.copied)
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
                else -> LazyColumn(contentPadding = PaddingValues(vertical = 8.dp)) {
                    items(list) { entry ->
                        Column(
                            Modifier
                                .fillMaxWidth()
                                .clickable {
                                    copy(context, entry.command)
                                    scope.launch { snackbar.showSnackbar(copied) }
                                }
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
                    }
                }
            }
        }
    }
}

fun copy(context: Context, text: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText("Shed", text))
}
