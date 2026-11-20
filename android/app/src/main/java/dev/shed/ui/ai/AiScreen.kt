package dev.shed.ui.ai

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.AccountTree
import androidx.compose.material.icons.filled.AddPhotoAlternate
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Drafts
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PhotoCamera
import androidx.compose.material.icons.filled.RateReview
import androidx.compose.material.icons.filled.Replay
import androidx.compose.material.icons.filled.Save
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.automirrored.filled.Undo
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.AiActivity
import dev.shed.api.AiAnswer
import dev.shed.api.AiAnswerQuestions
import dev.shed.api.AiEdit
import dev.shed.api.AiEntry
import dev.shed.api.AiQueued
import dev.shed.api.AiRevert
import dev.shed.api.AiSend
import dev.shed.api.AiSetUnsent
import dev.shed.api.AiSettings
import dev.shed.api.AiTab
import dev.shed.api.AiAddDraft
import dev.shed.api.Call
import dev.shed.api.Picture
import dev.shed.api.TabKind
import dev.shed.api.TabOnly
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.bench.MenuItem
import dev.shed.ui.canChange
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.LimitBars
import dev.shed.ui.common.Loading
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.decodePicture
import dev.shed.ui.common.findTab
import dev.shed.ui.common.nowSeconds
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.common.span
import dev.shed.ui.logbook.copy
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.workspaceState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Claude Code's permission modes: its value and the label. */
val MODES = listOf(
    "default" to R.string.mode_default,
    "acceptEdits" to R.string.mode_accept_edits,
    "auto" to R.string.mode_auto,
    "plan" to R.string.mode_plan,
    "bypassPermissions" to R.string.mode_full_auto,
)

/** What a long press on one of your messages offers. */
private data class UserAction(val index: Int, val text: String)

/** An AI tab: the conversation, the prompt, and everything about the session. */
@Composable
fun AiScreen(nav: NavController, ws: String, tabId: String) {
    val session = LocalSession.current
    val state = workspaceState(ws)
    val found = state?.findTab(tabId)
    val ai = (found?.second?.kind as? TabKind.Ai)?.content
    if (found == null || ai == null) {
        ShedScaffold(title = stringResource(R.string.tab_gone), onBack = { nav.popBackStack() }) {
            EmptyNote(stringResource(R.string.tab_gone_text), Modifier.padding(it))
        }
        return
    }
    val (bench, tab) = found
    val vm: AiVm = viewModel(key = "ai-$ws-$tabId") { AiVm(session.socket, ws, tabId) }
    val entries by vm.entries.collectAsState()
    val status by vm.status.collectAsState()
    val sessions by vm.sessions.collectAsState()
    val pictures by vm.pictures.collectAsState()
    val ended by vm.ended.collectAsState()
    val canChange = canChange(ws)
    val runner = rememberRunner()
    val shed = LocalShed.current
    var menu by remember { mutableStateOf(false) }
    var showSessions by rememberSaveable { mutableStateOf(false) }
    var showDrafts by rememberSaveable { mutableStateOf(false) }
    var userAction by remember { mutableStateOf<UserAction?>(null) }
    var editing by remember { mutableStateOf<UserAction?>(null) }
    var reverting by remember { mutableStateOf<UserAction?>(null) }
    var bigPicture by remember { mutableStateOf<ImageBitmap?>(null) }
    val context = LocalContext.current

    fun call(c: Call) = runner.run { session.socket.call(ws, c) }

    val currentKey = ai.session ?: ai.draftSession
    val title = sessions?.firstOrNull { it.id == (status?.session ?: ai.session) }?.title ?: tab.title
    val draftCount = ai.drafts.orEmpty().count { it.session == currentKey }

    ShedScaffold(
        title = title,
        subtitle = bench.name + " · " + tab.title,
        accent = benchColor(bench.color),
        onBack = { nav.popBackStack() },
        actions = {
            IconButton(onClick = { vm.loadSessions(); showSessions = true }) {
                Icon(Icons.Default.History, stringResource(R.string.ai_sessions))
            }
            IconButton(onClick = { nav.navigate(Routes.review(ws, tabId)) }) {
                Icon(Icons.Default.RateReview, stringResource(R.string.ai_review))
            }
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.more)) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    MenuItem(Icons.Default.Drafts, stringResource(R.string.ai_drafts_n, draftCount)) { menu = false; showDrafts = true }
                    MenuItem(Icons.Default.AccountTree, stringResource(R.string.git_title)) { menu = false; nav.navigate(Routes.git(ws, tabId)) }
                    MenuItem(Icons.Default.ContentCopy, stringResource(R.string.ai_copy_conversation)) {
                        menu = false
                        copy(context, conversationText(entries.orEmpty()))
                    }
                }
            }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).imePadding()) {
            StatusLine(ai, status?.let { it } , state.limits)
            val list = entries
            Box(Modifier.weight(1f)) {
                when {
                    list == null && ended -> EmptyNote(stringResource(R.string.ai_unavailable))
                    list == null -> Loading()
                    list.isEmpty() -> EmptyNote(stringResource(R.string.ai_empty))
                    else -> Transcript(
                        list,
                        pictures,
                        vm::picture,
                        canChange,
                        onUserLongPress = { i, text -> userAction = UserAction(i, text) },
                        onAnswer = { request, allow, always -> call(Call.AiAnswer(AiAnswer(tabId, request, allow, always))) },
                        onAnswers = { request, answers -> call(Call.AiAnswerQuestions(AiAnswerQuestions(tabId, request, answers))) },
                        onPicture = { bigPicture = it },
                    )
                }
            }
            if (canChange) Prompt(ws, tabId, ai, vm)
        }
    }

    if (showSessions) SessionsSheet(ws, tabId, vm, sessions, currentKey, canChange) { showSessions = false }
    if (showDrafts) DraftsSheet(ws, tabId, ai, currentKey, canChange) { showDrafts = false }
    userAction?.let { action ->
        AlertDialog(
            onDismissRequest = { userAction = null },
            title = { Text(stringResource(R.string.ai_your_message)) },
            text = { Text(action.text, maxLines = 6, overflow = TextOverflow.Ellipsis) },
            confirmButton = {
                Column {
                    if (canChange) {
                        TextButton(onClick = { userAction = null; editing = action }) {
                            Icon(Icons.Default.Edit, null); HSpace(8); Text(stringResource(R.string.ai_edit))
                        }
                        TextButton(onClick = {
                            userAction = null
                            call(Call.AiEdit(AiEdit(tabId, action.index.toUInt(), action.text, null)))
                        }) { Icon(Icons.Default.Replay, null); HSpace(8); Text(stringResource(R.string.ai_send_again)) }
                        TextButton(
                            onClick = { userAction = null; reverting = action },
                            colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                        ) { Icon(Icons.AutoMirrored.Filled.Undo, null); HSpace(8); Text(stringResource(R.string.ai_revert)) }
                    }
                    TextButton(onClick = { userAction = null; copy(context, action.text) }) {
                        Icon(Icons.Default.ContentCopy, null); HSpace(8); Text(stringResource(R.string.copy))
                    }
                }
            },
        )
    }
    editing?.let { action ->
        TextDialog(
            title = stringResource(R.string.ai_edit),
            initial = action.text,
            confirm = stringResource(R.string.send),
            onConfirm = { call(Call.AiEdit(AiEdit(tabId, action.index.toUInt(), it, null))) },
            onDismiss = { editing = null },
        )
    }
    reverting?.let { action ->
        AlertDialog(
            onDismissRequest = { reverting = null },
            title = { Text(stringResource(R.string.ai_revert_title)) },
            text = { Text(stringResource(R.string.ai_revert_text)) },
            confirmButton = {
                Column(horizontalAlignment = Alignment.End) {
                    TextButton(
                        onClick = { reverting = null; call(Call.AiRevert(AiRevert(tabId, action.index.toUInt(), true))) },
                        colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                    ) { Text(stringResource(R.string.ai_revert_files)) }
                    TextButton(
                        onClick = { reverting = null; call(Call.AiRevert(AiRevert(tabId, action.index.toUInt(), false))) },
                        colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                    ) { Text(stringResource(R.string.ai_revert_conversation)) }
                    TextButton(onClick = { reverting = null }) { Text(stringResource(R.string.cancel)) }
                }
            },
        )
    }
    bigPicture?.let { bitmap ->
        Dialog(onDismissRequest = { bigPicture = null }) {
            Image(bitmap, null, contentScale = ContentScale.Fit, modifier = Modifier.fillMaxWidth().clickable { bigPicture = null })
        }
    }
}

/** Working or waiting, since when; the cost and the context; the usage limits. */
@Composable
private fun StatusLine(ai: AiTab, status: dev.shed.api.AiStatus?, limits: dev.shed.api.Limits?) {
    val shed = LocalShed.current
    var now by remember { mutableLongStateOf(nowSeconds()) }
    val activity = status?.state ?: ai.activity
    LaunchedEffect(activity) {
        while (activity == AiActivity.Working) {
            delay(1000)
            now = nowSeconds()
        }
    }
    Column(Modifier.fillMaxWidth().background(shed.surface).padding(horizontal = 16.dp, vertical = 6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val (text, color) = when (activity) {
                AiActivity.Working -> {
                    val since = status?.since?.toLong()
                    stringResource(R.string.ai_working) + (since?.let { " · " + span((now - it).coerceAtLeast(0)) } ?: "") to shed.accent
                }
                AiActivity.Waiting -> stringResource(R.string.ai_waiting_for_you) to shed.warn
                AiActivity.Idle -> stringResource(R.string.ai_idle) to shed.textDim
            }
            Text(text, color = color, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
            val cost = status?.cost ?: 0.0
            if (cost > 0) Text("$" + "%.2f".format(cost), color = shed.textDim, style = MaterialTheme.typography.labelSmall)
            val context = status?.context ?: 0.0
            val max = status?.contextMax ?: 0.0
            if (context > 0) {
                HSpace(10)
                Text(
                    if (max > 0) stringResource(R.string.ai_context_of, k(context), k(max)) else stringResource(R.string.ai_context, k(context)),
                    color = shed.textDim,
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
        LimitBars(limits, Modifier.padding(top = 4.dp))
    }
}

private fun k(tokens: Double): String = when {
    tokens >= 1_000_000 -> "%.1fM".format(tokens / 1_000_000).replace(".0M", "M")
    tokens >= 1000 -> "${(tokens / 1000).toInt()}k"
    else -> tokens.toInt().toString()
}

@Composable
private fun Transcript(
    entries: List<AiEntry>,
    pictures: Map<String, ImageBitmap?>,
    loadPicture: (String) -> Unit,
    canChange: Boolean,
    onUserLongPress: (Int, String) -> Unit,
    onAnswer: (String, Boolean, Boolean) -> Unit,
    onAnswers: (String, List<String>) -> Unit,
    onPicture: (ImageBitmap) -> Unit,
) {
    val list = rememberLazyListState()
    var follow by remember { mutableStateOf(true) }
    LaunchedEffect(list.isScrollInProgress) {
        if (!list.isScrollInProgress) {
            val last = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            follow = last >= list.layoutInfo.totalItemsCount - 2
        }
    }
    val lastText = (entries.lastOrNull() as? AiEntry.Assistant)?.content?.text?.length ?: 0
    LaunchedEffect(entries.size, lastText) {
        if (follow && entries.isNotEmpty()) list.scrollToItem(entries.size - 1, Int.MAX_VALUE / 2)
    }
    LazyColumn(
        state = list,
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        itemsIndexed(entries) { index, entry ->
            when (entry) {
                is AiEntry.User -> UserEntry(entry.content, pictures, loadPicture, { onUserLongPress(index, entry.content.text) }, onPicture)
                is AiEntry.Assistant -> MarkdownView(entry.content.text)
                is AiEntry.Thinking -> ThinkingEntry(entry.content.text)
                is AiEntry.Tool -> ToolEntry(entry.content, pictures, loadPicture, onPicture)
                is AiEntry.Permission -> PermissionEntry(
                    entry.content,
                    canChange,
                    onAnswer = { allow, always -> onAnswer(entry.content.request, allow, always) },
                    onAnswers = { onAnswers(entry.content.request, it) },
                )
                is AiEntry.Notice -> NoticeEntry(entry.content.text, entry.content.error == true)
            }
        }
    }
}

/** The whole conversation as text, to copy. */
fun conversationText(entries: List<AiEntry>): String = entries.mapNotNull {
    when (it) {
        is AiEntry.User -> "> " + it.content.text.replace("\n", "\n> ")
        is AiEntry.Assistant -> it.content.text
        is AiEntry.Tool -> "    " + it.content.summary
        else -> null
    }
}.joinToString("\n\n")

@Composable
private fun Prompt(ws: String, tabId: String, ai: AiTab, vm: AiVm) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val context = LocalContext.current
    val runner = rememberRunner()
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    val info by vm.info.collectAsState()
    var text by rememberSaveable(tabId) { mutableStateOf(ai.unsent.orEmpty()) }
    var typed by remember { mutableStateOf(false) }
    val attached = remember { mutableStateListOf<Picture>() }
    var attachMenu by remember { mutableStateOf(false) }
    var editQueued by remember { mutableStateOf<Int?>(null) }
    var cameraTarget by remember { mutableStateOf<Uri?>(null) }
    val pictureFailed = stringResource(R.string.ai_picture_failed)

    fun call(c: Call) = runner.run { session.socket.call(ws, c) }

    fun attach(uri: Uri?) {
        if (uri == null) return
        scope.launch {
            val picture = runCatching { pictureForPrompt(context, uri) }.getOrNull()
            if (picture != null) attached.add(picture) else snackbar.showSnackbar(pictureFailed)
        }
    }

    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { attach(it) }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok -> if (ok) attach(cameraTarget) }

    // Text put in the prompt from elsewhere (a draft to edit) fills an empty field.
    LaunchedEffect(ai.unsent) {
        val unsent = ai.unsent.orEmpty()
        if (unsent != text && text.isEmpty()) {
            text = unsent
            typed = false
        }
    }
    // What is written and not sent is kept with the tab.
    LaunchedEffect(text) {
        if (!typed) return@LaunchedEffect
        delay(800)
        runCatching { session.socket.call(ws, Call.AiSetUnsent(AiSetUnsent(tabId, text))) }
    }

    val working = ai.activity != AiActivity.Idle
    Column(Modifier.fillMaxWidth().background(shed.surface).navigationBarsPadding()) {
        val queued = ai.queued.orEmpty()
        queued.forEachIndexed { i, q ->
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.ai_queued), color = shed.textDim, fontSize = 11.sp)
                HSpace(6)
                Text(q, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                IconButton(onClick = { editQueued = i }) { Icon(Icons.Default.Edit, stringResource(R.string.ai_edit), Modifier.size(18.dp)) }
                IconButton(onClick = { call(Call.AiQueued(AiQueued(tabId, i.toUInt(), q, true))) }) {
                    Icon(Icons.AutoMirrored.Filled.Send, stringResource(R.string.ai_send_now), Modifier.size(18.dp))
                }
                IconButton(onClick = { call(Call.AiQueued(AiQueued(tabId, i.toUInt(), "", null))) }) {
                    Icon(Icons.Default.Close, stringResource(R.string.remove), Modifier.size(18.dp), tint = shed.danger)
                }
            }
        }
        if (attached.isNotEmpty()) {
            Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                attached.forEachIndexed { i, p ->
                    val bitmap = remember(p) { decodePicture(p) }
                    Box {
                        bitmap?.let { Image(it, null, contentScale = ContentScale.Crop, modifier = Modifier.size(56.dp).clip(RoundedCornerShape(6.dp))) }
                        Icon(
                            Icons.Default.Close,
                            stringResource(R.string.remove),
                            tint = shed.background,
                            modifier = Modifier.align(Alignment.TopEnd).size(18.dp).clip(CircleShape).background(shed.text).clickable { attached.removeAt(i) },
                        )
                    }
                }
            }
        }
        // Slash commands, while one is being typed.
        if (text.startsWith("/") && ' ' !in text) {
            val matches = info.commands.filter { it.name.startsWith(text.drop(1)) }.take(6)
            for (cmd in matches) {
                Row(
                    Modifier.fillMaxWidth().clickable { text = "/${cmd.name} "; typed = true }.padding(horizontal = 16.dp, vertical = 6.dp),
                ) {
                    Text("/" + cmd.name, color = shed.accent)
                    HSpace(8)
                    Text(cmd.description.orEmpty(), color = shed.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 12.sp)
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.Bottom) {
            Box {
                IconButton(onClick = { attachMenu = true }) { Icon(Icons.Default.AddPhotoAlternate, stringResource(R.string.ai_attach)) }
                DropdownMenu(expanded = attachMenu, onDismissRequest = { attachMenu = false }) {
                    MenuItem(Icons.Default.AddPhotoAlternate, stringResource(R.string.ai_attach_picture)) {
                        attachMenu = false
                        pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                    }
                    MenuItem(Icons.Default.PhotoCamera, stringResource(R.string.ai_attach_photo)) {
                        attachMenu = false
                        val uri = cameraUri(context)
                        cameraTarget = uri
                        camera.launch(uri)
                    }
                }
            }
            OutlinedTextField(
                value = text,
                onValueChange = { text = it; typed = true },
                placeholder = { Text(stringResource(if (working) R.string.ai_prompt_queue else R.string.ai_prompt)) },
                maxLines = 8,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp).padding(vertical = 4.dp),
            )
            if (text.isNotBlank()) {
                IconButton(onClick = {
                    val keep = text
                    text = ""
                    typed = true
                    call(Call.AiAddDraft(AiAddDraft(tabId, keep, null)))
                }) { Icon(Icons.Default.Save, stringResource(R.string.ai_draft)) }
            }
            if (working && text.isBlank() && attached.isEmpty()) {
                IconButton(onClick = { call(Call.AiStop(TabOnly(tabId))) }) {
                    Icon(Icons.Default.Stop, stringResource(R.string.ai_stop), tint = shed.danger)
                }
            } else {
                IconButton(
                    enabled = text.isNotBlank() || attached.isNotEmpty(),
                    onClick = {
                        val send = AiSend(tabId, text, attached.toList().ifEmpty { null })
                        text = ""
                        typed = true
                        attached.clear()
                        call(Call.AiSend(send))
                    },
                ) { Icon(Icons.AutoMirrored.Filled.Send, stringResource(R.string.send), tint = shed.accent) }
            }
        }
        Settings(ws, tabId, ai, info)
    }

    editQueued?.let { i ->
        TextDialog(
            title = stringResource(R.string.ai_edit_queued),
            initial = ai.queued.orEmpty().getOrNull(i).orEmpty(),
            confirm = stringResource(R.string.save),
            allowEmpty = true,
            onConfirm = { call(Call.AiQueued(AiQueued(tabId, i.toUInt(), it, null))) },
            onDismiss = { editQueued = null },
        )
    }
}

/** Model, effort and mode, under the prompt. */
@Composable
private fun Settings(ws: String, tabId: String, ai: AiTab, info: dev.shed.api.AiInfo) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val runner = rememberRunner()
    var modelMenu by remember { mutableStateOf(false) }
    var effortMenu by remember { mutableStateOf(false) }
    var modeMenu by remember { mutableStateOf(false) }
    var fullAuto by remember { mutableStateOf(false) }
    fun set(settings: AiSettings) = runner.run { session.socket.call(ws, Call.AiSettings(settings)) }
    val models = info.models
    val model = models.firstOrNull { it.value == (ai.model ?: "default") }
    val modelName = model?.name ?: ai.model ?: stringResource(R.string.ai_default_model)
    val efforts = model?.efforts.orEmpty()
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box {
            AssistChip(onClick = { modelMenu = true }, label = { Text(modelName, fontSize = 12.sp) })
            DropdownMenu(expanded = modelMenu, onDismissRequest = { modelMenu = false }) {
                for (m in models) {
                    androidx.compose.material3.DropdownMenuItem(
                        text = {
                            Column {
                                Text(m.name + if (m.value == (ai.model ?: "default")) "  ✓" else "")
                                m.description?.takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 11.sp, color = shed.textDim) }
                            }
                        },
                        onClick = {
                            modelMenu = false
                            set(AiSettings(tabId, model = if (m.value == "default") "" else m.value))
                        },
                    )
                }
            }
        }
        if (efforts.isNotEmpty()) {
            Box {
                AssistChip(
                    onClick = { effortMenu = true },
                    label = { Text(stringResource(R.string.ai_effort, ai.effort ?: efforts.first()), fontSize = 12.sp) },
                )
                DropdownMenu(expanded = effortMenu, onDismissRequest = { effortMenu = false }) {
                    for (e in efforts) {
                        androidx.compose.material3.DropdownMenuItem(
                            text = { Text(e + if (e == ai.effort) "  ✓" else "") },
                            onClick = { effortMenu = false; set(AiSettings(tabId, effort = e)) },
                        )
                    }
                }
            }
        }
        Box {
            val label = MODES.firstOrNull { it.first == ai.mode }?.second?.let { stringResource(it) } ?: ai.mode
            AssistChip(
                onClick = { modeMenu = true },
                label = { Text(label, fontSize = 12.sp, color = if (ai.mode == "bypassPermissions") shed.danger else androidx.compose.ui.graphics.Color.Unspecified) },
            )
            DropdownMenu(expanded = modeMenu, onDismissRequest = { modeMenu = false }) {
                for ((value, name) in MODES) {
                    val danger = value == "bypassPermissions"
                    androidx.compose.material3.DropdownMenuItem(
                        text = {
                            Text(
                                stringResource(name) + (if (danger) "…" else "") + if (value == ai.mode) "  ✓" else "",
                                color = if (danger) shed.danger else androidx.compose.ui.graphics.Color.Unspecified,
                            )
                        },
                        onClick = {
                            modeMenu = false
                            if (danger) fullAuto = true else set(AiSettings(tabId, mode = value))
                        },
                    )
                }
            }
        }
    }
    if (fullAuto) {
        ConfirmDialog(
            title = stringResource(R.string.mode_full_auto_title),
            text = stringResource(R.string.mode_full_auto_text),
            confirm = stringResource(R.string.mode_full_auto),
            onConfirm = { set(AiSettings(tabId, mode = "bypassPermissions")) },
            onDismiss = { fullAuto = false },
        )
    }
}
