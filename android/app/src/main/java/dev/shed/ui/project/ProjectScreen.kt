package dev.shed.ui.project

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.InsertDriveFile
import androidx.compose.material.icons.filled.AccountTree
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.TravelExplore
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.Bench
import dev.shed.api.Call
import dev.shed.api.Event
import dev.shed.api.FileContent
import dev.shed.api.FileKind
import dev.shed.api.FileMatch
import dev.shed.api.FolderEntry
import dev.shed.api.Highlight
import dev.shed.api.ProjectPath
import dev.shed.api.ProjectTab
import dev.shed.api.ProjectTopic
import dev.shed.api.QuickOpen
import dev.shed.api.Reply
import dev.shed.api.SearchHits
import dev.shed.api.SearchTopic
import dev.shed.api.Tab as ApiTab
import dev.shed.api.Topic
import dev.shed.net.SubEvent
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.Loading
import dev.shed.ui.common.PictureView
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.charRange
import dev.shed.ui.common.textRange
import dev.shed.ui.common.textRanges
import dev.shed.ui.common.errorText
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono
import dev.shed.ui.theme.ShedColors
import dev.shed.ui.workspaceState
import kotlinx.coroutines.delay

/** A project tab: its files as a tree, quick open, a file viewer, search in files; Git from the top bar. */
@Composable
fun ProjectScreen(nav: NavController, ws: String, bench: Bench, tab: ApiTab, project: ProjectTab) {
    var page by rememberSaveable { mutableIntStateOf(0) }
    var file by rememberSaveable { mutableStateOf<String?>(null) }
    var line by rememberSaveable { mutableIntStateOf(0) }
    var quickOpen by rememberSaveable { mutableStateOf(false) }
    val home = workspaceState(ws)?.home.orEmpty()
    BackHandler(enabled = file != null) { file = null }
    val session = LocalSession.current
    val refresh = remember { mutableIntStateOf(0) }
    LaunchedEffect(ws, tab.id) {
        session.socket.subscribe(ws, Topic.Project(ProjectTopic(tab.id))).collect {
            if (it is SubEvent.Data && it.event is Event.FilesChanged) refresh.intValue++
        }
    }

    ShedScaffold(
        title = file?.substringAfterLast('/') ?: tab.title,
        subtitle = file ?: dev.shed.ui.common.tildePath(project.root, home),
        accent = benchColor(bench.color),
        onBack = { if (file != null) file = null else nav.popBackStack() },
        actions = {
            IconButton(onClick = { quickOpen = true }) { Icon(Icons.Default.Search, stringResource(R.string.project_quick_open)) }
            IconButton(onClick = { nav.navigate(Routes.git(ws, tab.id)) }) { Icon(Icons.Default.AccountTree, stringResource(R.string.git_title)) }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            val open = file
            if (open != null) {
                FileViewer(ws, tab.id, open, line, refresh.intValue)
            } else {
                PrimaryTabRow(selectedTabIndex = page) {
                    Tab(page == 0, { page = 0 }, text = { Text(stringResource(R.string.project_files)) })
                    Tab(page == 1, { page = 1 }, text = { Text(stringResource(R.string.project_search)) })
                }
                if (page == 0) {
                    FileTree(ws, tab.id, refresh.intValue) { file = it; line = 0 }
                } else {
                    SearchPage(ws, tab.id) { path, at -> file = path; line = at }
                }
            }
        }
    }
    if (quickOpen) {
        QuickOpenDialog(ws, tab.id, onPick = { file = it; line = 0 }, onDismiss = { quickOpen = false })
    }
}

private data class TreeRow(val path: String, val entry: FolderEntry, val depth: Int)

@Composable
private fun FileTree(ws: String, tab: String, version: Int, onOpen: (String) -> Unit) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val context = LocalContext.current
    val folders = remember { mutableStateMapOf<String, List<FolderEntry>>() }
    val expanded = remember { mutableStateListOf<String>() }
    var error by remember { mutableStateOf<String?>(null) }

    suspend fun load(path: String) {
        try {
            val reply = session.socket.call(ws, Call.ListFolder(ProjectPath(tab, path)))
            folders[path] = (reply as? Reply.Folder)?.content.orEmpty()
                .sortedWith(compareBy({ !it.dir }, { it.name.lowercase() }))
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
        }
    }
    LaunchedEffect(version) {
        load("")
        for (path in expanded.toList()) load(path)
    }
    val root = folders[""] ?: return if (error != null) EmptyNote(error!!) else Loading()
    val rows = buildList {
        fun walk(path: String, entries: List<FolderEntry>, depth: Int) {
            for (e in entries) {
                val child = if (path.isEmpty()) e.name else "$path/${e.name}"
                add(TreeRow(child, e, depth))
                if (e.dir && child in expanded) folders[child]?.let { walk(child, it, depth + 1) }
            }
        }
        walk("", root, 0)
    }
    val wanted = expanded.filter { it !in folders }
    LaunchedEffect(wanted) { for (path in wanted) load(path) }
    LazyColumn(Modifier.fillMaxSize()) {
        items(rows, key = { it.path }) { row ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable {
                        if (row.entry.dir) {
                            if (row.path in expanded) expanded.remove(row.path) else expanded.add(row.path)
                        } else {
                            onOpen(row.path)
                        }
                    }
                    .padding(start = (12 + row.depth * 16).dp, end = 12.dp, top = 8.dp, bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val dim = row.entry.ignored == true
                if (row.entry.dir) {
                    Icon(if (row.path in expanded) Icons.Default.ExpandMore else Icons.Default.ChevronRight, null, tint = shed.textDim)
                    Icon(Icons.Default.Folder, null, tint = if (dim) shed.textDim else shed.accent)
                } else {
                    HSpace(24)
                    Icon(Icons.AutoMirrored.Filled.InsertDriveFile, null, tint = shed.textDim)
                }
                HSpace(8)
                Text(row.entry.name, color = if (dim) shed.textDim else shed.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

/** A code token's colour by its kind (`keyword`, `string`…), from the theme's palette. */
fun highlightStyle(kind: String, shed: ShedColors): SpanStyle = when (kind.substringBefore('.')) {
    "keyword" -> SpanStyle(color = shed.ansi[5])
    "string", "escape" -> SpanStyle(color = shed.ansi[2])
    "comment" -> SpanStyle(color = shed.textDim, fontStyle = FontStyle.Italic)
    "function", "method" -> SpanStyle(color = shed.ansi[4])
    "type", "constructor" -> SpanStyle(color = shed.ansi[3])
    "number", "constant", "boolean" -> SpanStyle(color = shed.accent)
    "property", "variable", "field" -> SpanStyle(color = shed.ansi[6])
    "operator", "punctuation" -> SpanStyle(color = shed.textDim)
    "tag" -> SpanStyle(color = shed.ansi[1])
    "attribute", "label" -> SpanStyle(color = shed.ansi[3])
    else -> SpanStyle()
}

/** A line of code with its highlights (UTF-16 ranges) coloured. */
fun highlightLine(text: String, highlights: List<Highlight>, shed: ShedColors): AnnotatedString = buildAnnotatedString {
    append(text)
    for (h in highlights) {
        val r = textRange(text, h.start.toInt(), h.end.toInt()) ?: continue
        addStyle(highlightStyle(h.kind, shed), r.first, r.last + 1)
    }
}

@Composable
private fun FileViewer(ws: String, tab: String, path: String, line: Int, version: Int) {
    val session = LocalSession.current
    val context = LocalContext.current
    val shed = LocalShed.current
    var content by remember(path) { mutableStateOf<FileContent?>(null) }
    var error by remember(path) { mutableStateOf<String?>(null) }
    LaunchedEffect(path, version) {
        try {
            content = (session.socket.call(ws, Call.ReadFile(ProjectPath(tab, path))) as? Reply.File)?.content
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
        }
    }
    val c = content ?: return if (error != null) EmptyNote(error!!) else Loading()
    when (c.kind) {
        FileKind.Picture -> c.picture?.let { PictureView(it, Modifier.fillMaxWidth().padding(8.dp)) }
        FileKind.Binary -> EmptyNote(stringResource(R.string.project_binary))
        FileKind.TooLarge -> EmptyNote(stringResource(R.string.project_too_large))
        FileKind.Text -> {
            val text = c.text.orEmpty()
            val highlights = c.highlights.orEmpty()
            val lines = remember(text, highlights, shed) {
                text.split('\n').mapIndexed { i, l -> highlightLine(l, highlights.getOrElse(i) { emptyList() }, shed) }
            }
            val list = rememberLazyListState()
            LaunchedEffect(line) { if (line > 0) list.scrollToItem((line - 4).coerceAtLeast(0)) }
            val scroll = rememberScrollState()
            val gutter = (lines.size.toString().length * 8 + 8).dp
            Box(Modifier.fillMaxSize().horizontalScroll(scroll)) {
                LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                    itemsIndexed(lines) { i, l ->
                        Row(Modifier.padding(end = 16.dp)) {
                            Text(
                                (i + 1).toString(),
                                fontFamily = Mono,
                                fontSize = 12.sp,
                                color = if (i + 1 == line) shed.accent else shed.textDim,
                                modifier = Modifier.width(gutter).padding(start = 4.dp),
                            )
                            Text(l, fontFamily = Mono, fontSize = 12.sp, softWrap = false)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun QuickOpenDialog(ws: String, tab: String, onPick: (String) -> Unit, onDismiss: () -> Unit) {
    val session = LocalSession.current
    val shed = LocalShed.current
    var query by rememberSaveable { mutableStateOf("") }
    var matches by remember { mutableStateOf<List<FileMatch>>(emptyList()) }
    LaunchedEffect(query) {
        delay(150)
        matches = runCatching {
            (session.socket.call(ws, Call.QuickOpen(QuickOpen(tab, query, 50u))) as? Reply.Files)?.content.orEmpty()
        }.getOrDefault(emptyList())
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.project_quick_open)) },
        text = {
            Column {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    singleLine = true,
                    placeholder = { Text(stringResource(R.string.project_quick_open_hint)) },
                    modifier = Modifier.fillMaxWidth(),
                )
                LazyColumn(Modifier.heightIn(max = 400.dp)) {
                    items(matches, key = { it.path }) { m ->
                        val label = buildAnnotatedString {
                            append(m.path)
                            for (hit in m.hits) {
                                val r = charRange(m.path, hit.toInt()) ?: continue
                                addStyle(SpanStyle(color = shed.accent, fontWeight = FontWeight.Bold), r.first, r.last + 1)
                            }
                        }
                        Text(
                            label,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.fillMaxWidth().clickable {
                                onDismiss()
                                onPick(m.path)
                            }.padding(vertical = 8.dp),
                        )
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun SearchPage(ws: String, tab: String, onOpen: (String, Int) -> Unit) {
    val session = LocalSession.current
    val shed = LocalShed.current
    var query by rememberSaveable { mutableStateOf("") }
    var regex by rememberSaveable { mutableStateOf(false) }
    var caseSensitive by rememberSaveable { mutableStateOf(false) }
    var word by rememberSaveable { mutableStateOf(false) }
    val results = remember { mutableStateListOf<SearchHits>() }
    var done by remember { mutableStateOf<String?>(null) }
    val doneText = stringResource(R.string.search_done)
    val truncText = stringResource(R.string.search_truncated)
    LaunchedEffect(query, regex, caseSensitive, word) {
        results.clear()
        done = null
        if (query.length < 2) return@LaunchedEffect
        delay(300)
        val topic = Topic.Search(SearchTopic(tab, query, regex, caseSensitive, word, null, null))
        session.socket.subscribe(ws, topic).collect { ev ->
            when (ev) {
                is SubEvent.Data -> when (val e = ev.event) {
                    is Event.SearchHits -> results.add(e.content)
                    is Event.SearchDone -> done = e.content.error ?: if (e.content.truncated == true) truncText else doneText
                    else -> {}
                }
                is SubEvent.Ended -> done = ev.error?.message ?: doneText
            }
        }
    }
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            leadingIcon = { Icon(Icons.Default.TravelExplore, null) },
            placeholder = { Text(stringResource(R.string.search_hint)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
        )
        Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(regex, { regex = !regex }, { Text(".*", fontFamily = Mono) })
            FilterChip(caseSensitive, { caseSensitive = !caseSensitive }, { Text("Aa") })
            FilterChip(word, { word = !word }, { Text(stringResource(R.string.search_word)) })
        }
        done?.let { Text(it, color = shed.textDim, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 16.dp)) }
        LazyColumn(Modifier.fillMaxSize()) {
            for (file in results) {
                item(key = "f-" + file.path) {
                    Text(file.path, fontWeight = FontWeight.Medium, color = shed.accent, modifier = Modifier.padding(start = 12.dp, top = 10.dp, end = 12.dp))
                }
                items(file.hits, key = { "h-" + file.path + ":" + it.line }) { hit ->
                    val text = hit.text.trimEnd()
                    val label = buildAnnotatedString {
                        append(text)
                        for (r in textRanges(text, hit.ranges)) {
                            addStyle(SpanStyle(background = shed.accent.copy(alpha = 0.3f)), r.first, r.last + 1)
                        }
                    }
                    Row(Modifier.fillMaxWidth().clickable { onOpen(file.path, hit.line.toInt()) }.padding(horizontal = 12.dp, vertical = 4.dp)) {
                        Text(hit.line.toString(), fontFamily = Mono, fontSize = 12.sp, color = shed.textDim, modifier = Modifier.width(44.dp))
                        Text(label, fontFamily = Mono, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, color = Color.Unspecified)
                    }
                }
            }
        }
    }
}
