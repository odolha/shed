package dev.shed.ui.ai

import android.net.Uri
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.ui.res.painterResource
import dev.shed.ui.common.ListScrollbar
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.automirrored.filled.Undo
import androidx.compose.material.icons.filled.AccountTree
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AddPhotoAlternate
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Drafts
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PhotoCamera
import androidx.compose.material.icons.filled.RateReview
import androidx.compose.material.icons.filled.Replay
import androidx.compose.material.icons.filled.Save
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Tune
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.AiActivity
import dev.shed.api.AiAddDraft
import dev.shed.api.AiAnswer
import dev.shed.api.AiAnswerQuestions
import dev.shed.api.AiEdit
import dev.shed.api.AiEntry
import dev.shed.api.AiInfo
import dev.shed.api.AiQueued
import dev.shed.api.AiRevert
import dev.shed.api.AiSend
import dev.shed.api.AiSetUnsent
import dev.shed.api.AiStatus
import dev.shed.api.AiTab
import dev.shed.api.AiTimer
import dev.shed.api.AiTimerKind
import dev.shed.api.Call
import dev.shed.api.Limits
import dev.shed.api.MAX_UPLOAD
import dev.shed.api.Picture
import dev.shed.api.TabKind
import dev.shed.api.TabOnly
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.bench.MenuItem
import dev.shed.ui.canChange
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.LimitBars
import dev.shed.ui.common.LimitStrip
import dev.shed.ui.common.Loading
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.VSpace
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.decodePicture
import dev.shed.ui.common.errorText
import dev.shed.ui.common.findTab
import dev.shed.ui.common.hasLimits
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
    FULL_AUTO to R.string.mode_full_auto,
)

/** One of your messages, to edit or revert. */
private data class UserAction(val index: Int, val text: String)

/** What an AI tab shows: gathered by [AiScreen] (or a screenshot test) for [AiTabView]. */
class AiTabData(
    val ws: String,
    val tab: String,
    val title: String,
    val subtitle: String,
    val accent: Color?,
    val ai: AiTab,
    /** `null` while it loads. */
    val entries: List<AiEntry>?,
    val status: AiStatus?,
    val limits: Limits?,
    val info: AiInfo,
    val pictures: Map<String, ImageBitmap?>,
    /** The subscription ended (the workspace went away). */
    val ended: Boolean,
    val canChange: Boolean,
    val home: String,
    val drafts: Int,
)

/** What an AI tab does, for [AiTabView]. */
class AiTabActions(
    /** Sends a call to the workspace (failures are shown in the snackbar). */
    val send: suspend (Call) -> Unit,
    /** Keeps a file on the workspace for a prompt; answers its path there. */
    val upload: suspend (String, ByteArray) -> String,
    val loadPicture: (String) -> Unit,
    val back: () -> Unit,
    val sessions: () -> Unit,
    val review: () -> Unit,
    val drafts: () -> Unit,
    val git: () -> Unit,
    val copyAll: () -> Unit,
    /** Edit, Send Again or Revert on one of your messages (Copy and Select Text are the view's own). */
    val userAction: (UserMenu, Int, String) -> Unit,
    val picture: (ImageBitmap) -> Unit,
    val timer: (AiTimer) -> Unit,
)

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
    val info by vm.info.collectAsState()
    val ended by vm.ended.collectAsState()
    val canChange = canChange(ws)
    val runner = rememberRunner()
    val shed = LocalShed.current
    var showSessions by rememberSaveable { mutableStateOf(false) }
    var showDrafts by rememberSaveable { mutableStateOf(false) }
    // A draft to add at the end of the prompt (Merge into Prompt), until the prompt takes it.
    var mergeIn by remember { mutableStateOf<String?>(null) }
    var editing by remember { mutableStateOf<UserAction?>(null) }
    var reverting by remember { mutableStateOf<UserAction?>(null) }
    var bigPicture by remember { mutableStateOf<ImageBitmap?>(null) }
    val context = LocalContext.current
    var timer by remember { mutableStateOf<AiTimer?>(null) }
    // Runs' times and timers' countdowns: every second while one is in its
    // first (or last) minute, else every 30 s; nothing to count, no ticking.
    val now = remember { mutableLongStateOf(nowSeconds()) }
    LaunchedEffect(entries, status) {
        now.longValue = nowSeconds()
        while (true) {
            val every = tickEvery(entries.orEmpty(), status, nowSeconds()) ?: break
            delay(every * 1000)
            now.longValue = nowSeconds()
        }
    }

    fun call(c: Call) = runner.run { session.socket.call(ws, c) }

    val currentKey = ai.session ?: ai.draftSession
    val title = sessions?.firstOrNull { it.id == (status?.session ?: ai.session) }?.title ?: tab.title
    val data = AiTabData(
        ws = ws,
        tab = tabId,
        title = title,
        subtitle = bench.name + " · " + tab.title,
        accent = benchColor(bench.color),
        ai = ai,
        entries = entries,
        status = status,
        limits = state.limits,
        info = info,
        pictures = pictures,
        ended = ended,
        canChange = canChange,
        home = state.home.orEmpty(),
        drafts = ai.drafts.orEmpty().count { it.session == currentKey },
    )
    val actions = remember(ws, tabId, vm) {
        AiTabActions(
            send = { session.socket.call(ws, it) },
            upload = vm::upload,
            loadPicture = vm::picture,
            back = { nav.popBackStack() },
            sessions = { vm.loadSessions(); showSessions = true },
            review = { nav.navigate(Routes.review(ws, tabId)) },
            drafts = { showDrafts = true },
            git = { nav.navigate(Routes.git(ws, tabId)) },
            copyAll = { copy(context, conversationText(vm.entries.value.orEmpty())) },
            userAction = { action, i, text ->
                when (action) {
                    UserMenu.Edit -> editing = UserAction(i, text)
                    UserMenu.SendAgain -> call(Call.AiEdit(AiEdit(tabId, i.toUInt(), text, null)))
                    UserMenu.Revert -> reverting = UserAction(i, text)
                    UserMenu.Copy, UserMenu.Select -> {}
                }
            },
            picture = { bigPicture = it },
            timer = { timer = it },
        )
    }
    AiTabView(data, actions, { now.longValue }, mergeIn) { mergeIn = null }

    timer?.let { t ->
        AlertDialog(
            onDismissRequest = { timer = null },
            title = { Text(timerLabel(t, now.longValue)) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(timerDetail(t))
                    Text(stringResource(R.string.ai_timer_note), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
                }
            },
            confirmButton = { TextButton(onClick = { timer = null }) { Text(stringResource(R.string.close)) } },
        )
    }

    if (showSessions) SessionsSheet(ws, tabId, vm, sessions, currentKey, canChange) { showSessions = false }
    if (showDrafts) DraftsSheet(ws, tabId, ai, currentKey, canChange, onMerge = { mergeIn = it }) { showDrafts = false }
    editing?.let { action ->
        TextDialog(
            title = stringResource(R.string.ai_edit),
            initial = action.text,
            confirm = stringResource(R.string.send),
            multiLine = true,
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

/**
 * The AI tab as drawn: the top bar, the status and limits, the conversation
 * (opening at its end), what waits in the background, and the prompt.
 * While you type, the prompt takes all the room under the top bar, above
 * the keyboard (the conversation stays where it was, out of sight); putting
 * the keyboard away brings it all back.
 */
@Composable
fun AiTabView(data: AiTabData, on: AiTabActions, now: () -> Long, mergeIn: String? = null, onMerged: () -> Unit = {}) {
    val runner = rememberRunner()
    val context = LocalContext.current
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    var menu by remember { mutableStateOf(false) }
    var typing by remember { mutableStateOf(false) }
    var openRun by rememberSaveable { mutableStateOf<String?>(null) }
    var selecting by remember { mutableStateOf<String?>(null) }
    val copied = stringResource(R.string.copied)
    fun copyText(text: String) {
        copy(context, text)
        scope.launch { snackbar.showSnackbar(copied) }
    }
    val onUser: (UserMenu, Int, String) -> Unit = { action, i, text ->
        when (action) {
            UserMenu.Copy -> copyText(text)
            UserMenu.Select -> selecting = text
            else -> on.userAction(action, i, text)
        }
    }
    ShedScaffold(
        title = data.title,
        subtitle = data.subtitle,
        accent = data.accent,
        onBack = on.back,
        actions = {
            IconButton(onClick = on.sessions) { Icon(Icons.Default.History, stringResource(R.string.ai_sessions)) }
            IconButton(onClick = on.review) { Icon(Icons.Default.RateReview, stringResource(R.string.ai_review)) }
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.more)) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    MenuItem(Icons.Default.Drafts, stringResource(R.string.ai_drafts_n, data.drafts)) { menu = false; on.drafts() }
                    MenuItem(Icons.Default.AccountTree, stringResource(R.string.git_title)) { menu = false; on.git() }
                    MenuItem(Icons.Default.ContentCopy, stringResource(R.string.ai_copy_conversation)) { menu = false; on.copyAll() }
                }
            }
        },
    ) { padding ->
        // The prompt pads the navigation bar itself (in its colour). With the
        // keyboard up, this pads all of it and the prompt's padding (the
        // navigation bar, already counted in) comes to nothing: insets are
        // consumed, so excluding the bar here would leave the bottom that
        // much under the keyboard.
        Box(
            Modifier
                .fillMaxSize()
                .padding(top = padding.calculateTopPadding())
                .windowInsetsPadding(WindowInsets.ime),
        ) {
            // Typing: the prompt has the whole room. The conversation stays
            // composed (at no height), so it comes back scrolled where it was.
            val full = typing && data.canChange
            Column(Modifier.fillMaxSize()) {
                if (!full) StatusLine(data.ai, data.status, data.limits, compact = false)
                val list = data.entries
                Box(if (full) Modifier.height(0.dp).clipToBounds() else Modifier.weight(1f)) {
                    when {
                        list == null && data.ended -> EmptyNote(stringResource(R.string.ai_unavailable))
                        list == null -> Loading()
                        list.isEmpty() -> EmptyNote(stringResource(R.string.ai_empty))
                        // Another session: opened at its end, following, as a new one.
                        else -> key(data.status?.session) {
                            Transcript(
                            list,
                            data.status,
                            now,
                            data.pictures,
                            on.loadPicture,
                            data.canChange,
                            onUser = onUser,
                            onOpenRun = { openRun = it },
                            onAnswer = { request, allow, always -> runner.run { on.send(Call.AiAnswer(AiAnswer(data.tab, request, allow, always))) } },
                            onAnswers = { request, answers -> runner.run { on.send(Call.AiAnswerQuestions(AiAnswerQuestions(data.tab, request, answers))) } },
                            onPicture = on.picture,
                            navigation = !typing,
                            )
                        }
                    }
                }
                if (!full) BackgroundStrip(data.entries, data.status, now, folded = false, onTimer = on.timer, onRun = { openRun = it })
                if (data.canChange) {
                    Prompt(data, on, full, if (full) Modifier.weight(1f) else Modifier, mergeIn, onMerged) { typing = it }
                } else {
                    Spacer(Modifier.navigationBarsPadding())
                }
            }
        }
    }
    openRun?.let { id ->
        val tool = data.entries.orEmpty().firstNotNullOfOrNull { (it as? AiEntry.Tool)?.content?.takeIf { t -> t.id == id && t.run != null } }
        if (tool == null) {
            openRun = null
        } else {
            RunSheet(tool, data.status?.let { it.state != AiActivity.Idle } == true, now()) { openRun = null }
        }
    }
    selecting?.let { text ->
        AlertDialog(
            onDismissRequest = { selecting = null },
            title = { Text(stringResource(R.string.ai_message_select_title)) },
            text = {
                SelectionContainer(Modifier.verticalScroll(rememberScrollState())) {
                    Text(text, style = MaterialTheme.typography.bodyMedium)
                }
            },
            confirmButton = {
                TextButton(onClick = { selecting = null; copyText(text) }) {
                    Icon(Icons.Default.ContentCopy, null); HSpace(8); Text(stringResource(R.string.ai_copy_all))
                }
            },
            dismissButton = { TextButton(onClick = { selecting = null }) { Text(stringResource(R.string.close)) } },
        )
    }
}

/**
 * Working or waiting, since when; the cost and the context; the usage
 * limits as two thin bars (tap for their percentages and resets). [compact]
 * (while typing): one line, the bars small at its end.
 */
@Composable
private fun StatusLine(ai: AiTab, status: AiStatus?, limits: Limits?, compact: Boolean) {
    val shed = LocalShed.current
    var now by remember { mutableLongStateOf(nowSeconds()) }
    var details by rememberSaveable { mutableStateOf(false) }
    val activity = status?.state ?: ai.activity
    LaunchedEffect(activity) {
        while (activity == AiActivity.Working) {
            delay(1000)
            now = nowSeconds()
        }
    }
    val withLimits = hasLimits(limits)
    val detailsLabel = stringResource(if (details) R.string.limits_hide else R.string.limits_show)
    Column(
        Modifier
            .fillMaxWidth()
            .background(shed.surface)
            .then(if (withLimits) Modifier.clickable(onClickLabel = detailsLabel) { details = !details } else Modifier)
            .padding(horizontal = 16.dp, vertical = if (compact) 4.dp else 6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val (text, color) = when (activity) {
                AiActivity.Working -> {
                    val since = status?.since?.toLong()
                    stringResource(R.string.ai_working) + (since?.let { " · " + span((now - it).coerceAtLeast(0)) } ?: "") to shed.accent
                }
                AiActivity.Waiting -> stringResource(R.string.ai_waiting_for_you) to shed.warn
                AiActivity.Idle -> stringResource(R.string.ai_idle) to shed.textDim
            }
            Text(text, color = color, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
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
                    maxLines = 1,
                )
            }
            if (withLimits && compact && !details) {
                HSpace(10)
                LimitStrip(limits, Modifier.width(56.dp), labelled = false)
            }
        }
        if (withLimits && !compact && !details) LimitStrip(limits, Modifier.padding(top = 6.dp))
        if (details) LimitBars(limits, Modifier.padding(top = 6.dp))
    }
}

private fun k(tokens: Double): String = when {
    tokens >= 1_000_000 -> "%.1fM".format(tokens / 1_000_000).replace(".0M", "M")
    tokens >= 1000 -> "${(tokens / 1000).toInt()}k"
    else -> tokens.toInt().toString()
}

/**
 * The conversation: it opens at its latest entry and follows new output
 * while you are at the end; scrolling up stops that, and ↓ goes back. ▲ ▼
 * jump between your messages.
 */
@Composable
private fun Transcript(
    entries: List<AiEntry>,
    status: AiStatus?,
    now: () -> Long,
    pictures: Map<String, ImageBitmap?>,
    loadPicture: (String) -> Unit,
    canChange: Boolean,
    onUser: (UserMenu, Int, String) -> Unit,
    onOpenRun: (String) -> Unit,
    onAnswer: (String, Boolean, Boolean) -> Unit,
    onAnswers: (String, List<String>) -> Unit,
    onPicture: (ImageBitmap) -> Unit,
    /** The ▲ ▼ between your messages (not while typing: the room is short). */
    navigation: Boolean,
) {
    val shed = LocalShed.current
    val scope = rememberCoroutineScope()
    // The item after the last entry: scrolling to it shows the very end.
    val list = rememberLazyListState(initialFirstVisibleItemIndex = entries.size)
    val follow = rememberSaveable(saver = Follow.Saver) { Follow() }
    val slack = with(LocalDensity.current) { 24.dp.roundToPx() }
    fun endNow(): Boolean {
        val info = list.layoutInfo
        val last = info.visibleItemsInfo.lastOrNull()
        return atEnd(list.canScrollForward, last?.index, info.totalItemsCount, (last?.offset ?: 0) + (last?.size ?: 0), info.viewportEndOffset - info.afterContentPadding, slack)
    }
    fun toEnd() = (list.layoutInfo.totalItemsCount - 1).coerceAtLeast(0)
    val moreBelow by remember { derivedStateOf { !endNow() } }
    LaunchedEffect(list) {
        list.interactionSource.interactions.collect { if (it is DragInteraction.Start) follow.dragged() }
    }
    LaunchedEffect(list) {
        snapshotFlow { list.isScrollInProgress }.collect { moving -> if (!moving) follow.settled(endNow()) }
    }
    // New output, a picture loading, the keyboard opening: back to the end while following.
    LaunchedEffect(list) {
        snapshotFlow { follow.shouldScroll(list.canScrollForward) }.collect { if (it) list.scrollToItem(toEnd()) }
    }
    val previous by remember(entries) { derivedStateOf { previousPrompt(entries, list.firstVisibleItemIndex, list.firstVisibleItemScrollOffset > 0) } }
    val next by remember(entries) { derivedStateOf { nextPrompt(entries, list.firstVisibleItemIndex) } }
    val hasPrompts = remember(entries) { promptIndices(entries).isNotEmpty() }
    val showNav = navigation && hasPrompts && (list.canScrollBackward || list.canScrollForward)
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            state = list,
            modifier = Modifier.fillMaxSize(),
            // Room at the end for ▲ ▼, so the latest text is never under them.
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 12.dp, bottom = if (navigation && hasPrompts) NAV_ROOM else 4.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            itemsIndexed(entries) { index, entry ->
                when (entry) {
                    is AiEntry.User -> UserEntry(entry.content, pictures, loadPicture, canChange, { onUser(it, index, entry.content.text) }, onPicture)
                    is AiEntry.Assistant -> MarkdownView(entry.content.text)
                    is AiEntry.Thinking -> ThinkingEntry(entry.content.text)
                    is AiEntry.Tool -> ToolEntry(entry.content, status, now, pictures, loadPicture, onPicture, onOpenRun)
                    is AiEntry.Permission -> PermissionEntry(
                        entry.content,
                        canChange,
                        onAnswer = { allow, always -> onAnswer(entry.content.request, allow, always) },
                        onAnswers = { onAnswers(entry.content.request, it) },
                    )
                    is AiEntry.Notice -> NoticeEntry(entry.content.text, entry.content.error == true)
                    is AiEntry.Note -> NoteEntry(entry.content)
                    // An entry a newer workspace sends that this version does not know: kept in its place, shown as nothing.
                    AiEntry.Unknown -> {}
                }
            }
            item(key = "end") { Spacer(Modifier.height(1.dp)) }
        }
        ListScrollbar(list, Modifier.align(Alignment.TopEnd)) { follow.dragged() }
        // ▲ ▼ and ↓ side by side in the bottom corner: small, see-through, out of the text where they can be.
        Row(
            Modifier.align(Alignment.BottomEnd).padding(end = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (showNav) {
                Surface(
                    shape = RoundedCornerShape(16.dp),
                    color = shed.surface.copy(alpha = 0.72f),
                    border = BorderStroke(1.dp, shed.divider.copy(alpha = 0.6f)),
                ) {
                    Row {
                        IconButton(
                            modifier = Modifier.size(width = 40.dp, height = 32.dp),
                            enabled = previous != null,
                            onClick = { previous?.let { follow.away(); scope.launch { list.animateScrollToItem(it) } } },
                        ) { Icon(Icons.Default.KeyboardArrowUp, stringResource(R.string.ai_previous_prompt), Modifier.size(20.dp)) }
                        IconButton(
                            modifier = Modifier.size(width = 40.dp, height = 32.dp),
                            enabled = next != null || moreBelow,
                            onClick = {
                                val to = next
                                if (to != null) {
                                    follow.away()
                                    scope.launch { list.animateScrollToItem(to) }
                                } else {
                                    follow.toLatest()
                                    scope.launch { list.animateScrollToItem(toEnd()) }
                                }
                            },
                        ) { Icon(Icons.Default.KeyboardArrowDown, stringResource(R.string.ai_next_prompt), Modifier.size(20.dp)) }
                    }
                }
            }
            if (follow.showJump(moreBelow)) {
                SmallFloatingActionButton(
                    onClick = { follow.toLatest(); scope.launch { list.animateScrollToItem(toEnd()) } },
                    containerColor = shed.accent,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                ) { Icon(Icons.Default.ArrowDownward, stringResource(R.string.ai_to_latest)) }
            }
        }
    }
}

/** The room kept below the last entry for ▲ ▼. */
private val NAV_ROOM = 44.dp

/** The whole conversation as text, to copy. */
fun conversationText(entries: List<AiEntry>): String = entries.mapNotNull {
    when (it) {
        is AiEntry.User -> "> " + it.content.text.replace("\n", "\n> ")
        is AiEntry.Assistant -> it.content.text
        is AiEntry.Tool -> "    " + it.content.summary
        else -> null
    }
}.joinToString("\n\n")

/**
 * What goes on in the background, above the prompt: sub-agents and commands
 * running (first), then what Claude set to happen later (wake-ups, scheduled
 * jobs, watches). One thing shows as itself; several as a line saying how
 * many of each, which opens to the list. A run opens its sheet; a timer what
 * it tells Claude. [folded] (while typing): back to one line.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun BackgroundStrip(
    entries: List<AiEntry>?,
    status: AiStatus?,
    now: () -> Long,
    folded: Boolean,
    onTimer: (AiTimer) -> Unit,
    onRun: (String) -> Unit,
) {
    val timers = status?.timers.orEmpty()
    val runs = remember(entries) { runningTools(entries.orEmpty()) }
    var unfolded by remember { mutableStateOf(false) }
    LaunchedEffect(folded) { if (folded) unfolded = false }
    val count = timers.size + runs.size
    if (count == 0) return
    val shed = LocalShed.current
    val ink = shed.ink(shed.accent)
    val agents = runs.count { it.isAgent }
    val summary = backgroundSummary(agents, runs.size - agents, timers.size)
    @Composable
    fun SummaryLine(open: Boolean) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 36.dp)
                .clip(RoundedCornerShape(8.dp))
                .clickable(onClickLabel = stringResource(if (open) R.string.ai_strip_hide else R.string.ai_timers_show)) { unfolded = !open }
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (agents > 0) {
                Icon(painterResource(R.drawable.ic_agent), null, tint = ink, modifier = Modifier.size(16.dp))
            } else {
                Icon(if (runs.isNotEmpty()) Icons.Default.Terminal else Icons.Default.Schedule, null, tint = ink, modifier = Modifier.size(14.dp))
            }
            HSpace(8)
            Text(summary, color = ink, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Icon(if (open) Icons.Default.ExpandLess else Icons.Default.ExpandMore, null, tint = shed.textDim, modifier = Modifier.size(18.dp))
        }
    }
    Column(
        Modifier.fillMaxWidth().background(shed.surface).padding(horizontal = 12.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (count > 1) {
            SummaryLine(unfolded)
            if (!unfolded) return@Column
        }
        for (tool in runs) {
            val run = tool.run ?: continue
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 36.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .border(1.dp, shed.accent.copy(alpha = 0.4f), RoundedCornerShape(8.dp))
                    .clickable(onClickLabel = stringResource(R.string.ai_run_open)) { onRun(tool.id) }
                    .padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ToolIcon(tool, ink, 14.dp)
                HSpace(6)
                Text(runTime(run, now()), color = ink, fontSize = 12.sp, maxLines = 1)
                HSpace(8)
                Text(runTitle(tool), color = shed.text, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
        }
        for (t in timers) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 36.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .border(1.dp, shed.accent.copy(alpha = 0.4f), RoundedCornerShape(8.dp))
                    .combinedClickable(onClick = { onTimer(t) }, onLongClick = { onTimer(t) })
                    .padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(if (t.kind == AiTimerKind.Watch) Icons.Default.Visibility else Icons.Default.Schedule, null, tint = ink, modifier = Modifier.size(14.dp))
                HSpace(6)
                Text(timerLabel(t, now()), color = ink, fontSize = 12.sp, maxLines = 1)
                HSpace(8)
                Text(t.what, color = shed.text, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
        }
    }
}

/** A file going with the prompt: its name, and where the workspace keeps it once uploaded. */
private data class PromptFile(val key: Long, val name: String, val path: String? = null)

/**
 * The prompt: **+** (attach, keep as a draft), the text (three lines at
 * most; [full] while typing: all of the room [modifier] gives it, the text
 * scrolling in it), the settings sheet's icon, Send (or Stop).
 */
@Composable
private fun Prompt(data: AiTabData, on: AiTabActions, full: Boolean, modifier: Modifier, mergeIn: String?, onMerged: () -> Unit, onTyping: (Boolean) -> Unit) {
    val shed = LocalShed.current
    val context = LocalContext.current
    val runner = rememberRunner()
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    val focusManager = LocalFocusManager.current
    val ai = data.ai
    val tabId = data.tab
    var text by rememberSaveable(tabId) { mutableStateOf(ai.unsent.orEmpty()) }
    var typed by remember { mutableStateOf(false) }
    var focused by remember { mutableStateOf(false) }
    val attached = remember { mutableStateListOf<Picture>() }
    var plusMenu by remember { mutableStateOf(false) }
    var settings by rememberSaveable { mutableStateOf(false) }
    var editQueued by remember { mutableStateOf<Int?>(null) }
    var cameraTarget by remember { mutableStateOf<Uri?>(null) }
    val pictureFailed = stringResource(R.string.ai_picture_failed)
    val files = remember { mutableStateListOf<PromptFile>() }
    var fileKey by remember { mutableLongStateOf(0L) }

    fun call(c: Call) = runner.run { on.send(c) }

    // The keyboard put away (back): done typing, the prompt folds.
    val imeVisible = WindowInsets.isImeVisible
    var imeSeen by remember { mutableStateOf(false) }
    LaunchedEffect(imeVisible) {
        if (imeVisible) {
            imeSeen = true
        } else if (imeSeen) {
            imeSeen = false
            focusManager.clearFocus()
        }
    }

    fun attach(uri: Uri?) {
        if (uri == null) return
        scope.launch {
            val picture = runCatching { pictureForPrompt(context, uri) }.getOrNull()
            if (picture != null) attached.add(picture) else snackbar.showSnackbar(pictureFailed)
        }
    }

    // Any file: a picture goes as a picture; the rest is kept on the workspace
    // and goes as its path there, for Claude to read.
    fun attachFile(uri: Uri) {
        scope.launch {
            val picked = runCatching { pickedFile(context, uri) }.getOrElse {
                snackbar.showSnackbar(context.getString(R.string.ai_file_unreadable))
                return@launch
            }
            if (picked.mime?.startsWith("image/") == true) {
                val picture = runCatching { pictureForPrompt(context, uri) }.getOrNull()
                if (picture != null) {
                    attached.add(picture)
                    return@launch
                }
            }
            val max = MAX_UPLOAD.toLong()
            fun tooBig(size: Long) = context.getString(
                R.string.ai_file_too_big,
                picked.name,
                Formatter.formatShortFileSize(context, size),
                Formatter.formatShortFileSize(context, max),
            )
            if (picked.size != null && picked.size > max) {
                snackbar.showSnackbar(tooBig(picked.size))
                return@launch
            }
            val key = fileKey++
            files.add(PromptFile(key, picked.name))
            try {
                val bytes = readPicked(context, uri, max)
                val path = on.upload(picked.name, bytes)
                val i = files.indexOfFirst { it.key == key }
                if (i >= 0) files[i] = files[i].copy(path = path)
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                files.removeAll { it.key == key }
                snackbar.showSnackbar(
                    if (e is TooBigException) tooBig(max + 1) else context.getString(R.string.ai_file_failed, picked.name, errorText(context, e)),
                )
            }
        }
    }

    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { attach(it) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris -> uris.forEach(::attachFile) }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { ok -> if (ok) attach(cameraTarget) }

    // Text put in the prompt from elsewhere (a draft to edit) fills an empty field.
    LaunchedEffect(ai.unsent) {
        val unsent = ai.unsent.orEmpty()
        if (unsent != text && text.isEmpty()) {
            text = unsent
            typed = false
        }
    }
    // A draft merged in goes at the end of what is written.
    LaunchedEffect(mergeIn) {
        val draft = mergeIn ?: return@LaunchedEffect
        text = mergedDraft(text, draft)
        typed = true
        onMerged()
    }
    // What is written and not sent is kept with the tab.
    LaunchedEffect(text) {
        if (!typed) return@LaunchedEffect
        delay(800)
        runCatching { on.send(Call.AiSetUnsent(AiSetUnsent(tabId, text))) }
    }

    val working = ai.activity != AiActivity.Idle
    Column(modifier.fillMaxWidth().background(shed.surface).navigationBarsPadding()) {
        val queued = ai.queued.orEmpty()
        queued.forEachIndexed { i, q ->
            Row(Modifier.fillMaxWidth().padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
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
                    Box(Modifier.size(64.dp)) {
                        bitmap?.let { Image(it, null, contentScale = ContentScale.Crop, modifier = Modifier.size(56.dp).align(Alignment.BottomStart).clip(RoundedCornerShape(6.dp))) }
                        // A 32 dp target over the corner, its mark small.
                        Box(
                            Modifier.align(Alignment.TopEnd).size(32.dp).clip(CircleShape).clickable(onClickLabel = stringResource(R.string.remove)) { attached.removeAt(i) },
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(
                                Icons.Default.Close,
                                stringResource(R.string.remove),
                                tint = shed.background,
                                modifier = Modifier.size(18.dp).clip(CircleShape).background(shed.text),
                            )
                        }
                    }
                }
            }
        }
        if (files.isNotEmpty()) {
            Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                for (f in files) {
                    Row(
                        Modifier.clip(RoundedCornerShape(8.dp)).border(1.dp, shed.divider, RoundedCornerShape(8.dp)).padding(start = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        if (f.path == null) {
                            CircularProgressIndicator(Modifier.size(12.dp).semantics { contentDescription = context.getString(R.string.ai_uploading, f.name) }, strokeWidth = 1.5.dp)
                        } else {
                            Icon(Icons.Default.AttachFile, null, tint = shed.textDim, modifier = Modifier.size(14.dp))
                        }
                        HSpace(4)
                        Text(f.name, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 200.dp), color = if (f.path == null) shed.textDim else shed.text)
                        IconButton(onClick = { files.removeAll { it.key == f.key } }, modifier = Modifier.size(40.dp)) {
                            Icon(Icons.Default.Close, stringResource(R.string.remove), Modifier.size(16.dp))
                        }
                    }
                }
            }
        }
        // Slash commands, while one is being typed.
        if (text.startsWith("/") && ' ' !in text) {
            val matches = data.info.commands.filter { it.name.startsWith(text.drop(1)) }.take(6)
            for (cmd in matches) {
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 40.dp).clickable { text = "/${cmd.name} "; typed = true }.padding(horizontal = 16.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("/" + cmd.name, color = shed.accent)
                    HSpace(8)
                    Text(cmd.description.orEmpty(), color = shed.textDim, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 12.sp)
                }
            }
        }
        // While typing, the text has the whole width and the buttons go under
        // it; the field keeps its place in the tree either way, so its focus.
        val plusButton: @Composable () -> Unit = {
            Box(Modifier.padding(bottom = 4.dp)) {
                IconButton(onClick = { plusMenu = true }) { Icon(Icons.Default.Add, stringResource(R.string.ai_prompt_more)) }
                DropdownMenu(expanded = plusMenu, onDismissRequest = { plusMenu = false }) {
                    MenuItem(Icons.Default.AttachFile, stringResource(R.string.ai_attach_file)) {
                        plusMenu = false
                        pickFiles.launch(arrayOf("*/*"))
                    }
                    MenuItem(Icons.Default.AddPhotoAlternate, stringResource(R.string.ai_attach_picture)) {
                        plusMenu = false
                        pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                    }
                    MenuItem(Icons.Default.PhotoCamera, stringResource(R.string.ai_attach_photo)) {
                        plusMenu = false
                        val uri = cameraUri(context)
                        cameraTarget = uri
                        camera.launch(uri)
                    }
                    if (text.isNotBlank()) {
                        MenuItem(Icons.Default.Save, stringResource(R.string.ai_draft)) {
                            plusMenu = false
                            val keep = text
                            text = ""
                            typed = true
                            call(Call.AiAddDraft(AiAddDraft(tabId, keep, null)))
                        }
                    }
                }
            }
        }
        val settingsButton: @Composable () -> Unit = {
            Box(Modifier.padding(bottom = 4.dp)) {
                IconButton(onClick = { focusManager.clearFocus(); settings = true }) {
                    Icon(
                        Icons.Default.Tune,
                        stringResource(R.string.ai_settings),
                        tint = if (ai.mode == FULL_AUTO) shed.danger else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
        val sendButton: @Composable () -> Unit = {
            Box(Modifier.padding(bottom = 4.dp)) {
                if (working && text.isBlank() && attached.isEmpty() && files.isEmpty()) {
                    IconButton(onClick = { call(Call.AiStop(TabOnly(tabId))) }) {
                        Icon(Icons.Default.Stop, stringResource(R.string.ai_stop), tint = shed.danger)
                    }
                } else {
                    val canSend = text.isNotBlank() || attached.isNotEmpty() || files.isNotEmpty()
                    IconButton(
                        enabled = canSend,
                        onClick = {
                            // Files go once the workspace has them, as their paths, for Claude to read.
                            if (files.any { it.path == null }) {
                                scope.launch { snackbar.showSnackbar(context.getString(R.string.ai_files_uploading)) }
                                return@IconButton
                            }
                            val send = AiSend(tabId, withAttachedFiles(text, files.mapNotNull { it.path }), attached.toList().ifEmpty { null })
                            text = ""
                            typed = true
                            attached.clear()
                            files.clear()
                            call(Call.AiSend(send))
                        },
                    ) { Icon(Icons.AutoMirrored.Filled.Send, stringResource(R.string.send), tint = if (canSend) shed.accent else shed.textDim) }
                }
            }
        }
        Row(Modifier.fillMaxWidth().then(if (full) Modifier.weight(1f) else Modifier).padding(horizontal = 4.dp), verticalAlignment = Alignment.Bottom) {
            if (!full) plusButton()
            OutlinedTextField(
                value = text,
                onValueChange = { text = it; typed = true },
                placeholder = { Text(stringResource(if (working) R.string.ai_prompt_queue else R.string.ai_prompt), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                minLines = 1,
                maxLines = if (full) Int.MAX_VALUE else 3,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                modifier = Modifier
                    .weight(1f)
                    .then(if (full) Modifier.fillMaxHeight().padding(horizontal = 4.dp) else Modifier.heightIn(min = 48.dp))
                    .padding(vertical = 4.dp)
                    .onFocusChanged {
                        if (focused != it.isFocused) {
                            focused = it.isFocused
                            onTyping(it.isFocused)
                        }
                    },
            )
            if (!full) {
                settingsButton()
                sendButton()
            }
        }
        if (full) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                plusButton()
                Spacer(Modifier.weight(1f))
                settingsButton()
                sendButton()
            }
        }
    }

    if (settings) {
        AiSettingsSheet(data.ws, tabId, ai, data.info, data.home, set = { call(Call.AiSettings(it)) }) { settings = false }
    }
    editQueued?.let { i ->
        TextDialog(
            title = stringResource(R.string.ai_edit_queued),
            initial = ai.queued.orEmpty().getOrNull(i).orEmpty(),
            confirm = stringResource(R.string.save),
            allowEmpty = true,
            multiLine = true,
            onConfirm = { call(Call.AiQueued(AiQueued(tabId, i.toUInt(), it, null))) },
            onDismiss = { editQueued = null },
        )
    }
}
