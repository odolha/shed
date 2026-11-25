package dev.shed.ui.git

import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.automirrored.filled.CallSplit
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CloudDownload
import androidx.compose.material.icons.filled.CloudUpload
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Remove
import androidx.compose.material.icons.automirrored.filled.Undo
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.Branches
import dev.shed.api.Call
import dev.shed.api.Commit
import dev.shed.api.CommitFile
import dev.shed.api.Diff
import dev.shed.api.DiffSource
import dev.shed.api.GitBranch
import dev.shed.api.GitCommit
import dev.shed.api.GitCommitRef
import dev.shed.api.GitDiff
import dev.shed.api.GitFile
import dev.shed.api.GitHunk
import dev.shed.api.GitLog
import dev.shed.api.GitPaths
import dev.shed.api.GitPush
import dev.shed.api.GitStatus
import dev.shed.api.HunkAction
import dev.shed.api.ProjectTopic
import dev.shed.api.Reply
import dev.shed.api.TabOnly
import dev.shed.api.Topic
import dev.shed.net.SubEvent
import dev.shed.ui.LocalSession
import dev.shed.ui.canChange
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.DiffView
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.Loading
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.SectionTitle
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.ago
import dev.shed.ui.common.errorText
import dev.shed.ui.common.findTab
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono
import dev.shed.ui.workspaceState
import kotlinx.coroutines.launch

/** What a discard asks about: a file's changes, or one hunk. */
private data class Discard(val path: String, val hunk: Int? = null, val header: String = "")

/** A project's (or AI tab's folder's) Git: changes, staging by hunk, commits, history, branches, sync. */
@Composable
fun GitScreen(nav: NavController, ws: String, tab: String) {
    val session = LocalSession.current
    val context = LocalContext.current
    val runner = rememberRunner()
    val snackbar = LocalSnackbar.current
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val title = workspaceState(ws)?.findTab(tab)?.second?.title ?: ""
    val canChange = canChange(ws)
    var page by rememberSaveable { mutableIntStateOf(0) }
    var refresh by remember { mutableIntStateOf(0) }
    var status by remember { mutableStateOf<GitStatus?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var pulled by remember { mutableStateOf(false) }

    LaunchedEffect(refresh) {
        try {
            status = (session.socket.call(ws, Call.GitStatus(TabOnly(tab))) as? Reply.GitStatus)?.content
            error = null
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
        } finally {
            pulled = false
        }
    }
    LaunchedEffect(ws, tab) {
        session.socket.subscribe(ws, Topic.Project(ProjectTopic(tab))).collect { if (it is SubEvent.Data) refresh++ }
    }

    fun sync(call: Call) {
        busy = true
        scope.launch {
            try {
                val reply = session.socket.call(ws, call, 120_000)
                (reply as? Reply.Text)?.content?.trim()?.takeIf { it.isNotEmpty() }?.let { snackbar.showSnackbar(it.lines().last()) }
            } catch (e: Exception) {
                if (e is kotlinx.coroutines.CancellationException) throw e
                snackbar.showSnackbar(errorText(context, e))
            } finally {
                busy = false
                refresh++
            }
        }
    }

    ShedScaffold(
        title = stringResource(R.string.git_title),
        subtitle = title,
        onBack = { nav.popBackStack() },
        actions = { IconButton(onClick = { refresh++ }) { Icon(Icons.Default.Refresh, stringResource(R.string.refresh)) } },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
            PrimaryTabRow(selectedTabIndex = page) {
                Tab(page == 0, { page = 0 }, text = { Text(stringResource(R.string.git_changes)) })
                Tab(page == 1, { page = 1 }, text = { Text(stringResource(R.string.git_history)) })
                Tab(page == 2, { page = 2 }, text = { Text(stringResource(R.string.git_branches)) })
            }
            error?.let { Text(it, color = LocalShed.current.danger, modifier = Modifier.padding(12.dp)) }
            val s = status
            // Pulling down looks again (as Refresh does).
            PullToRefreshBox(isRefreshing = pulled, onRefresh = { pulled = true; refresh++ }, modifier = Modifier.fillMaxSize()) {
                when {
                    s == null -> Loading()
                    !s.repo -> EmptyNote(stringResource(R.string.git_not_repo))
                    page == 0 -> Changes(ws, tab, s, canChange, busy, ::sync, refresh) { refresh++ }
                    page == 1 -> History(ws, tab, refresh)
                    else -> BranchesPage(ws, tab, canChange, refresh) { refresh++ }
                }
            }
        }
    }
}

@Composable
private fun Changes(
    ws: String,
    tab: String,
    status: GitStatus,
    canChange: Boolean,
    busy: Boolean,
    sync: (Call) -> Unit,
    version: Int,
    changed: () -> Unit,
) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val runner = rememberRunner()
    var message by rememberSaveable { mutableStateOf("") }
    var amend by rememberSaveable { mutableStateOf(false) }
    var discard by remember { mutableStateOf<Discard?>(null) }
    val open = remember { mutableStateMapOf<String, Boolean>() }
    val staged = status.files.filter { it.staged.isNotBlank() && it.staged != "?" && it.conflict != true }
    val unstaged = status.files.filter { (it.unstaged.isNotBlank() || it.staged == "?") && it.conflict != true }
    val conflicts = status.files.filter { it.conflict == true }

    fun call(c: Call) = runner.run {
        session.socket.call(ws, c)
        changed()
    }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 32.dp)) {
        item {
            Column(Modifier.padding(16.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.AutoMirrored.Filled.CallSplit, null, tint = shed.accent)
                    HSpace(8)
                    Text(status.branch ?: stringResource(R.string.git_detached), fontWeight = FontWeight.Medium)
                    status.upstream?.let { Text("  → $it", color = shed.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
                val ahead = status.ahead?.toInt() ?: 0
                val behind = status.behind?.toInt() ?: 0
                if (ahead > 0 || behind > 0) {
                    Text(stringResource(R.string.git_ahead_behind, ahead, behind), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
                }
                status.operation?.let { Text(stringResource(R.string.git_operation, it), color = shed.warn) }
                if (canChange) {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                        OutlinedButton(enabled = !busy, onClick = { sync(Call.GitFetch(TabOnly(tab))) }) {
                            Icon(Icons.Default.CloudDownload, null); HSpace(6); Text(stringResource(R.string.git_fetch))
                        }
                        OutlinedButton(enabled = !busy, onClick = { sync(Call.GitPull(TabOnly(tab))) }) {
                            Icon(Icons.Default.Download, null); HSpace(6); Text(stringResource(R.string.git_pull))
                        }
                        OutlinedButton(enabled = !busy, onClick = { sync(Call.GitPush(GitPush(tab, null))) }) {
                            Icon(Icons.Default.CloudUpload, null); HSpace(6); Text(stringResource(R.string.git_push))
                        }
                    }
                }
            }
        }
        if (canChange && (staged.isNotEmpty() || amend)) {
            item {
                Column(Modifier.padding(horizontal = 16.dp)) {
                    OutlinedTextField(
                        value = message,
                        onValueChange = { message = it },
                        label = { Text(stringResource(R.string.git_message)) },
                        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                        modifier = Modifier.fillMaxWidth(),
                        minLines = 2,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(amend, { amend = it })
                        Text(stringResource(R.string.git_amend), modifier = Modifier.weight(1f))
                        Button(
                            enabled = message.isNotBlank() && !busy,
                            onClick = {
                                runner.run {
                                    session.socket.call(ws, Call.GitCommit(GitCommit(tab, message.trim(), amend)))
                                    message = ""
                                    amend = false
                                    changed()
                                }
                            },
                        ) {
                            Icon(Icons.Default.Check, null); HSpace(6); Text(stringResource(R.string.git_commit))
                        }
                    }
                }
            }
        }
        if (status.files.isEmpty()) {
            item { Text(stringResource(R.string.git_clean), color = shed.textDim, modifier = Modifier.padding(16.dp)) }
        }
        fileSection(R.string.git_conflicts, conflicts, DiffSource.Unstaged, open, ws, tab, canChange, version, ::call) { discard = it }
        fileSection(R.string.git_staged, staged, DiffSource.Staged, open, ws, tab, canChange, version, ::call) { discard = it }
        fileSection(R.string.git_unstaged, unstaged, DiffSource.Unstaged, open, ws, tab, canChange, version, ::call) { discard = it }
    }

    discard?.let { d ->
        ConfirmDialog(
            title = stringResource(if (d.hunk == null) R.string.git_discard_file_title else R.string.git_discard_hunk_title),
            text = stringResource(R.string.git_discard_text, d.path),
            confirm = stringResource(R.string.git_discard),
            onConfirm = {
                if (d.hunk == null) {
                    call(Call.GitDiscard(GitPaths(tab, listOf(d.path))))
                } else {
                    call(Call.GitHunk(GitHunk(tab, d.path, DiffSource.Unstaged, d.hunk.toUInt(), d.header, HunkAction.Discard)))
                }
            },
            onDismiss = { discard = null },
        )
    }
}

private fun LazyListScope.fileSection(
    title: Int,
    files: List<GitFile>,
    source: DiffSource,
    open: MutableMap<String, Boolean>,
    ws: String,
    tab: String,
    canChange: Boolean,
    version: Int,
    call: (Call) -> Unit,
    discard: (Discard) -> Unit,
) {
    if (files.isEmpty()) return
    item(key = "title-$title") { SectionTitle(androidx.compose.ui.res.stringResource(title) + " (${files.size})") }
    items(files, key = { "$title-${it.path}" }) { file ->
        val key = "$title-${file.path}"
        val staged = source == DiffSource.Staged
        val shed = LocalShed.current
        Column {
            Row(
                Modifier.fillMaxWidth().clickable { open[key] = open[key] != true }.padding(start = 16.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val letter = if (staged) file.staged else file.unstaged.ifBlank { file.staged }
                Text(letter, fontFamily = Mono, color = statusColor(letter), modifier = Modifier.width(20.dp))
                Text(file.path, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).padding(vertical = 10.dp))
                if (canChange) {
                    if (staged) {
                        IconButton(onClick = { call(Call.GitUnstage(GitPaths(tab, listOf(file.path)))) }) {
                            Icon(Icons.Default.Remove, androidx.compose.ui.res.stringResource(R.string.git_unstage))
                        }
                    } else {
                        IconButton(onClick = { discard(Discard(file.path)) }) {
                            Icon(Icons.AutoMirrored.Filled.Undo, androidx.compose.ui.res.stringResource(R.string.git_discard), tint = shed.danger)
                        }
                        IconButton(onClick = { call(Call.GitStage(GitPaths(tab, listOf(file.path)))) }) {
                            Icon(Icons.Default.Add, androidx.compose.ui.res.stringResource(R.string.git_stage))
                        }
                    }
                }
            }
            if (open[key] == true) {
                FileDiff(ws, tab, file.path, source, canChange, version, call, discard)
            }
        }
    }
}

@Composable
fun statusColor(letter: String) = LocalShed.current.let {
    when (letter.trim()) {
        "A", "?" -> it.added
        "D" -> it.removed
        "M", "R" -> it.warn
        "U" -> it.danger
        else -> it.textDim
    }
}

@Composable
private fun FileDiff(
    ws: String,
    tab: String,
    path: String,
    source: DiffSource,
    canChange: Boolean,
    version: Int,
    call: (Call) -> Unit,
    discard: (Discard) -> Unit,
) {
    val session = LocalSession.current
    val context = LocalContext.current
    val shed = LocalShed.current
    var diff by remember { mutableStateOf<Diff?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(path, source, version) {
        try {
            diff = (session.socket.call(ws, Call.GitDiff(GitDiff(tab, path, source))) as? Reply.Diff)?.content
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
        }
    }
    error?.let { Text(it, color = shed.danger, modifier = Modifier.padding(8.dp)) }
    val d = diff ?: return
    DiffView(d, hunkActions = if (!canChange || source is DiffSource.Commit) null else { index, hunk ->
        if (source == DiffSource.Staged) {
            TextButton(onClick = { call(Call.GitHunk(GitHunk(tab, path, source, index.toUInt(), hunk.header, HunkAction.Unstage))) }) {
                Text(stringResource(R.string.git_unstage))
            }
        } else {
            TextButton(
                onClick = { discard(Discard(path, index, hunk.header)) },
                colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
            ) { Text(stringResource(R.string.git_discard)) }
            TextButton(onClick = { call(Call.GitHunk(GitHunk(tab, path, source, index.toUInt(), hunk.header, HunkAction.Stage))) }) {
                Text(stringResource(R.string.git_stage))
            }
        }
    })
}

@Composable
private fun History(ws: String, tab: String, version: Int) {
    val session = LocalSession.current
    val context = LocalContext.current
    val shed = LocalShed.current
    var commits by remember { mutableStateOf<List<Commit>?>(null) }
    var openCommit by remember { mutableStateOf<String?>(null) }
    var files by remember { mutableStateOf<List<CommitFile>>(emptyList()) }
    var openFile by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(version) {
        commits = runCatching {
            (session.socket.call(ws, Call.GitLog(GitLog(tab, null, 200u))) as? Reply.Commits)?.content.orEmpty()
        }.getOrDefault(emptyList())
    }
    LaunchedEffect(openCommit) {
        val hash = openCommit ?: return@LaunchedEffect
        files = runCatching {
            (session.socket.call(ws, Call.GitCommitFiles(GitCommitRef(tab, hash))) as? Reply.CommitFiles)?.content.orEmpty()
        }.getOrDefault(emptyList())
    }
    val list = commits ?: return Loading()
    LazyColumn(Modifier.fillMaxSize()) {
        items(list, key = { it.hash }) { commit ->
            Column(
                Modifier.fillMaxWidth().clickable {
                    openCommit = if (openCommit == commit.hash) null else commit.hash
                    openFile = null
                }.padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                Text(commit.subject, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(
                    "${commit.short} · ${commit.author} · ${ago(context, commit.time.toLong())}",
                    color = shed.textDim,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            if (openCommit == commit.hash) {
                for (f in files) {
                    Row(
                        Modifier.fillMaxWidth().clickable { openFile = if (openFile == f.path) null else f.path }.padding(start = 32.dp, end = 16.dp, top = 6.dp, bottom = 6.dp),
                    ) {
                        Text(f.status, fontFamily = Mono, color = statusColor(f.status), modifier = Modifier.width(20.dp))
                        Text(f.path, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    if (openFile == f.path) {
                        FileDiff(ws, tab, f.path, DiffSource.Commit(commit.hash), false, version, {}, {})
                    }
                }
            }
        }
    }
}

@Composable
private fun BranchesPage(ws: String, tab: String, canChange: Boolean, version: Int, changed: () -> Unit) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val runner = rememberRunner()
    var branches by remember { mutableStateOf<Branches?>(null) }
    var creating by remember { mutableStateOf(false) }
    LaunchedEffect(version) {
        branches = runCatching {
            (session.socket.call(ws, Call.GitBranches(TabOnly(tab))) as? Reply.Branches)?.content
        }.getOrNull() ?: Branches(null, emptyList(), emptyList())
    }
    val b = branches ?: return Loading()
    LazyColumn(Modifier.fillMaxSize()) {
        if (canChange) {
            item {
                TextButton(onClick = { creating = true }, modifier = Modifier.padding(8.dp)) {
                    Icon(Icons.Default.Add, null); HSpace(6); Text(stringResource(R.string.git_new_branch))
                }
            }
        }
        item { SectionTitle(stringResource(R.string.git_local)) }
        items(b.local) { name ->
            BranchRow(name, name == b.current, canChange) {
                runner.run { session.socket.call(ws, Call.GitSwitch(GitBranch(tab, name))); changed() }
            }
        }
        if (b.remote.isNotEmpty()) {
            item { SectionTitle(stringResource(R.string.git_remote)) }
            items(b.remote) { name ->
                BranchRow(name, false, canChange) {
                    runner.run { session.socket.call(ws, Call.GitSwitch(GitBranch(tab, name))); changed() }
                }
            }
        }
    }
    if (creating) {
        TextDialog(
            title = stringResource(R.string.git_new_branch),
            initial = "",
            confirm = stringResource(R.string.create),
            onConfirm = { name -> runner.run { session.socket.call(ws, Call.GitCreateBranch(GitBranch(tab, name))); changed() } },
            onDismiss = { creating = false },
        )
    }
}

@Composable
private fun BranchRow(name: String, current: Boolean, canChange: Boolean, onSwitch: () -> Unit) {
    val shed = LocalShed.current
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(
            name,
            color = if (current) shed.accent else shed.text,
            fontWeight = if (current) FontWeight.Medium else FontWeight.Normal,
            modifier = Modifier.weight(1f),
        )
        if (!current && canChange) TextButton(onClick = onSwitch) { Text(stringResource(R.string.git_switch)) }
    }
}
