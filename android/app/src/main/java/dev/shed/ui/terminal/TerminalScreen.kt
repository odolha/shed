package dev.shed.ui.terminal

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.MenuBook
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.AspectRatio
import androidx.compose.material.icons.filled.ViewColumn
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.Bench
import dev.shed.api.Call
import dev.shed.api.Input
import dev.shed.api.InputItem
import dev.shed.api.Key
import dev.shed.api.Mouse
import dev.shed.api.MouseButton
import dev.shed.api.MouseKind
import dev.shed.api.PaneOnly
import dev.shed.api.Reply
import dev.shed.api.Scrollback
import dev.shed.api.Span
import dev.shed.api.Tab
import dev.shed.api.TakeSize
import dev.shed.api.Topic
import dev.shed.api.ScreenTopic
import dev.shed.net.NexusSocket
import dev.shed.net.SubEvent
import dev.shed.api.Event
import dev.shed.shed
import dev.shed.ui.LocalSession
import dev.shed.ui.Routes
import dev.shed.ui.bench.MenuItem
import dev.shed.ui.canChange
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.benchColor
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono
import dev.shed.ui.theme.ShedColors
import dev.shed.ui.theme.terminalColor
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/** A pane's screen, kept while the pane is shown. */
class TerminalVm(private val socket: NexusSocket, private val ws: String, val pane: String) : ViewModel() {
    val model = TerminalModel()
    private val _version = MutableStateFlow(0)
    val version: StateFlow<Int> = _version
    private val _ended = MutableStateFlow(false)
    val ended: StateFlow<Boolean> = _ended
    private var loading = false

    init {
        viewModelScope.launch {
            var first = true
            socket.subscribe(ws, Topic.Screen(ScreenTopic(pane))).collect { ev ->
                when (ev) {
                    is SubEvent.Data -> (ev.event as? Event.Screen)?.let {
                        if (first || it.content.full && _ended.value) model.clearHistory()
                        first = false
                        _ended.value = false
                        model.apply(it.content)
                        _version.value = model.version
                    }
                    is SubEvent.Ended -> _ended.value = true
                }
            }
        }
    }

    /** Fetches older lines, if there are any. */
    fun loadMore(onLoaded: (Int) -> Unit) {
        if (loading || model.moreHistory == 0) return
        loading = true
        viewModelScope.launch {
            try {
                val from = model.historyLines.size.toUInt()
                val reply = socket.call(ws, Call.Scrollback(Scrollback(pane, from, 200u)))
                if (reply is Reply.Lines && reply.content.isNotEmpty()) {
                    model.prependHistory(reply.content)
                    _version.value = model.version
                    onLoaded(reply.content.size)
                }
            } catch (_: Exception) {
            } finally {
                loading = false
            }
        }
    }
}

private val KEYS = listOf("Escape" to "Esc", "Tab" to "Tab", "Left" to "←", "Up" to "↑", "Down" to "↓", "Right" to "→",
    "|" to "|", "~" to "~", "/" to "/", "-" to "-", "Backspace" to "⌫", "Enter" to "⏎")

/** A terminal or remote tab: its active pane's screen, re-wrapped to the phone, and a way to type. */
@Composable
fun TerminalScreen(nav: NavController, ws: String, bench: Bench, tab: Tab) {
    val panes = tab.panes.orEmpty()
    var chosen by rememberSaveable(tab.id) { mutableStateOf<String?>(null) }
    val pane = panes.firstOrNull { it.id == chosen } ?: panes.firstOrNull { it.id == tab.activePane } ?: panes.firstOrNull()
    val session = LocalSession.current
    val shed = LocalShed.current
    val app = LocalContext.current.shed
    val runner = rememberRunner()
    val canChange = canChange(ws)
    var paneMenu by remember { mutableStateOf(false) }
    var takeSize by rememberSaveable(pane?.id) { mutableStateOf(false) }

    ShedScaffold(
        title = tab.title,
        subtitle = pane?.let { it.tool ?: it.title ?: it.cwd },
        accent = benchColor(bench.color),
        onBack = { nav.popBackStack() },
        actions = {
            if (panes.size > 1) {
                Box {
                    IconButton(onClick = { paneMenu = true }) { Icon(Icons.Default.ViewColumn, stringResource(R.string.terminal_panes)) }
                    DropdownMenu(expanded = paneMenu, onDismissRequest = { paneMenu = false }) {
                        panes.forEachIndexed { i, p ->
                            MenuItem(Icons.Default.ViewColumn, "${i + 1}. " + (p.tool ?: p.title ?: p.cwd)) {
                                paneMenu = false
                                chosen = p.id
                            }
                        }
                    }
                }
            }
            if (canChange && pane != null) {
                TooltipBox(
                    positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
                    tooltip = { PlainTooltip { Text(stringResource(R.string.terminal_take_size)) } },
                    state = rememberTooltipState(),
                ) {
                    IconToggleButton(checked = takeSize, onCheckedChange = { takeSize = it }) {
                        Icon(
                            Icons.Default.AspectRatio,
                            stringResource(R.string.terminal_take_size),
                            tint = if (takeSize) shed.accent else shed.textDim,
                        )
                    }
                }
            }
            IconButton(onClick = { nav.navigate(Routes.logbook(ws, tab.id)) }) {
                Icon(Icons.AutoMirrored.Filled.MenuBook, stringResource(R.string.logbook_title))
            }
        },
    ) { padding ->
        if (pane == null) {
            EmptyNote(stringResource(R.string.terminal_no_pane), Modifier.padding(padding))
            return@ShedScaffold
        }
        val vm: TerminalVm = viewModel(key = "term-${pane.id}") { TerminalVm(session.socket, ws, pane.id) }
        fun send(items: List<InputItem>) = runner.run { session.socket.call(ws, Call.Input(Input(pane.id, items))) }
        Column(Modifier.fillMaxSize().padding(padding).imePadding()) {
            val onMouse: ((List<Mouse>) -> Unit)? = if (canChange) { m -> send(m.map { InputItem.Mouse(it) }) } else null
            TerminalView(vm, Modifier.weight(1f), takeSize, onMouse) { cols, rows ->
                runner.run { session.socket.call(ws, Call.TakeSize(TakeSize(pane.id, cols.toUInt(), rows.toUInt()))) }
            }
            if (canChange) InputBar(::send)
        }
        val paneId = pane.id
        DisposableEffect(paneId, takeSize) {
            onDispose {
                if (takeSize) {
                    app.scope.launch { runCatching { session.socket.call(ws, Call.ReleaseSize(PaneOnly(paneId))) } }
                }
            }
        }
    }
}

@Composable
private fun TerminalView(
    vm: TerminalVm,
    modifier: Modifier,
    takeSize: Boolean,
    onMouse: ((List<Mouse>) -> Unit)?,
    onSize: (Int, Int) -> Unit,
) {
    val shed = LocalShed.current
    val version by vm.version.collectAsState()
    val ended by vm.ended.collectAsState()
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val fontSize = 13.sp
    val style = TextStyle(fontFamily = Mono, fontSize = fontSize, lineHeight = 16.sp, color = shed.terminalForeground)
    val cell = remember(style) { measurer.measure("MMMMMMMMMM", style).size }
    val charW = cell.width / 10f
    val lineH = with(density) { 16.sp.toPx() }
    val model = vm.model
    Surface(modifier.fillMaxWidth(), color = shed.terminalBackground) {
        BoxWithConstraints(Modifier.fillMaxSize().padding(horizontal = 4.dp)) {
            val widthPx = with(density) { maxWidth.toPx() }
            val heightPx = with(density) { maxHeight.toPx() }
            val cols = (widthPx / charW).toInt().coerceAtLeast(10)
            val rows = (heightPx / lineH).toInt().coerceAtLeast(4)
            LaunchedEffect(takeSize, cols, rows) { if (takeSize) onSize(cols, rows) }
            if (version == 0) {
                EmptyNote(stringResource(if (ended) R.string.terminal_ended else R.string.terminal_waiting))
                return@BoxWithConstraints
            }
            if (model.alt) {
                AltScreen(model, style, widthPx, charW, lineH, shed, onMouse)
            } else {
                NormalScreen(vm, version, cols, style, shed)
            }
        }
    }
}

@Composable
private fun NormalScreen(vm: TerminalVm, version: Int, cols: Int, style: TextStyle, shed: ShedColors) {
    val model = vm.model
    val rows = remember(version, cols, shed) {
        Rewrap.rewrap(model.historyLines + model.visibleScreen(), cols).map { styled(it, shed) }
    }
    val list = rememberLazyListState()
    var follow by remember { mutableStateOf(true) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(list.isScrollInProgress) {
        if (!list.isScrollInProgress) {
            val last = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            follow = last >= list.layoutInfo.totalItemsCount - 2
        }
    }
    LaunchedEffect(rows.size, version) {
        if (follow && rows.isNotEmpty()) list.scrollToItem(rows.size - 1)
    }
    val atTop by remember { derivedStateOf { list.firstVisibleItemIndex == 0 && !follow } }
    LaunchedEffect(atTop) {
        if (atTop) {
            val before = rows.size
            vm.loadMore { _ ->
                scope.launch {
                    // Keep what was at the top where it was.
                    kotlinx.coroutines.delay(16)
                    val added = (Rewrap.rewrap(model.historyLines + model.visibleScreen(), cols).size - before).coerceAtLeast(0)
                    list.scrollToItem(added)
                }
            }
        }
    }
    LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
        items(rows) { Text(it, style = style, softWrap = false, maxLines = 1) }
    }
}

@Composable
private fun AltScreen(
    model: TerminalModel,
    style: TextStyle,
    widthPx: Float,
    charW: Float,
    lineH: Float,
    shed: ShedColors,
    onMouse: ((List<Mouse>) -> Unit)?,
) {
    var fit by rememberSaveable { mutableStateOf(true) }
    val scale = if (fit) (widthPx / (model.cols * charW)).coerceIn(0.4f, 1f) else 1f
    val small = style.copy(fontSize = style.fontSize * scale, lineHeight = style.lineHeight * scale)
    val lines = model.visibleScreen().map { styled(it.spans, shed) }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            FilterChip(
                selected = fit,
                onClick = { fit = !fit },
                label = { Text(stringResource(R.string.terminal_fit), fontSize = 11.sp) },
            )
        }
        val scroll = rememberScrollState()
        val mouse = if (onMouse != null && model.mouse) {
            Modifier.mouseInput(model, charW * scale, lineH * scale, onMouse)
        } else {
            Modifier
        }
        Column(Modifier.fillMaxSize().then(if (fit) Modifier else Modifier.horizontalScroll(scroll)).then(mouse)) {
            for (line in lines) Text(line, style = small, softWrap = false, maxLines = 1)
        }
    }
}

/**
 * The program asked for the mouse (`Screen.mouse`): a tap is a left click at
 * its cell, a vertical drag turns the wheel a notch per row dragged (finger
 * down: wheel up, as content follows the finger).
 */
@Composable
private fun Modifier.mouseInput(model: TerminalModel, cellW: Float, cellH: Float, onMouse: (List<Mouse>) -> Unit): Modifier {
    val send by rememberUpdatedState(onMouse)
    fun cell(at: Offset): Pair<UInt, UInt> {
        val col = (at.x / cellW).toInt().coerceIn(0, model.cols - 1)
        val row = (at.y / cellH).toInt().coerceIn(0, model.rows - 1)
        return col.toUInt() to row.toUInt()
    }
    return pointerInput(cellW, cellH) {
        detectTapGestures { at ->
            val (col, row) = cell(at)
            send(listOf(Mouse(MouseKind.Press, MouseButton.Left, col, row), Mouse(MouseKind.Release, MouseButton.Left, col, row)))
        }
    }.pointerInput(cellW, cellH) {
        var dragged = 0f
        detectVerticalDragGestures(onDragStart = { dragged = 0f }) { change, dy ->
            change.consume()
            dragged += dy
            val notches = (dragged / cellH).toInt()
            if (notches != 0) {
                dragged -= notches * cellH
                val (col, row) = cell(change.position)
                val kind = if (notches > 0) MouseKind.WheelUp else MouseKind.WheelDown
                send(List(kotlin.math.abs(notches)) { Mouse(kind, null, col, row) })
            }
        }
    }
}

/** Spans as styled text, in the theme's terminal colours. */
fun styled(spans: List<Span>, shed: ShedColors): AnnotatedString = buildAnnotatedString {
    for (span in spans) {
        var fg = terminalColor(span.fg, shed)
        var bg = terminalColor(span.bg, shed)
        if (span.inverse == true) {
            val f = fg ?: shed.terminalForeground
            fg = bg ?: shed.terminalBackground
            bg = f
        }
        if (span.bold == true && span.fg != null && span.fg!!.toIntOrNull()?.let { it < 8 } == true) {
            fg = shed.ansi[span.fg!!.toInt() + 8]
        }
        if (span.dim == true) fg = (fg ?: shed.terminalForeground).copy(alpha = 0.6f)
        val decoration = when {
            span.underline == true && span.strike == true -> TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough))
            span.underline == true -> TextDecoration.Underline
            span.strike == true -> TextDecoration.LineThrough
            else -> null
        }
        val style = SpanStyle(
            color = fg ?: Color.Unspecified,
            background = bg ?: Color.Unspecified,
            fontWeight = if (span.bold == true) FontWeight.Bold else null,
            fontStyle = if (span.italic == true) FontStyle.Italic else null,
            textDecoration = decoration,
        )
        pushStyle(style)
        append(span.text)
        pop()
    }
}

@Composable
private fun InputBar(send: (List<InputItem>) -> Unit) {
    val shed = LocalShed.current
    var text by rememberSaveable { mutableStateOf("") }
    var ctrl by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }

    fun key(name: String) {
        if (name.length == 1 && !ctrl && !alt) {
            send(listOf(InputItem.Text(name)))
        } else {
            send(listOf(InputItem.Key(Key(name, ctrl, alt, null))))
        }
        ctrl = false
        alt = false
    }

    Column(Modifier.fillMaxWidth().background(shed.surface).navigationBarsPadding()) {
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            KeyChip("Ctrl", ctrl) { ctrl = !ctrl }
            KeyChip("Alt", alt) { alt = !alt }
            for ((name, label) in KEYS) KeyChip(label, false) { key(name) }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp)) {
            OutlinedTextField(
                value = text,
                onValueChange = { new ->
                    if ((ctrl || alt) && new.length == text.length + 1 && new.startsWith(text)) {
                        key(new.last().lowercase())
                    } else {
                        text = new
                    }
                },
                placeholder = { Text(stringResource(R.string.terminal_type)) },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = Mono),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onSend = {
                    send(if (text.isEmpty()) listOf(InputItem.Key(Key("Enter"))) else listOf(InputItem.Text(text), InputItem.Key(Key("Enter"))))
                    text = ""
                }),
                modifier = Modifier.weight(1f),
            )
            IconButton(onClick = {
                send(if (text.isEmpty()) listOf(InputItem.Key(Key("Enter"))) else listOf(InputItem.Text(text), InputItem.Key(Key("Enter"))))
                text = ""
            }) { Icon(Icons.AutoMirrored.Filled.Send, stringResource(R.string.send), tint = shed.accent) }
        }
    }
}

@Composable
private fun KeyChip(label: String, on: Boolean, onClick: () -> Unit) {
    FilterChip(
        selected = on,
        onClick = onClick,
        label = { Text(label, fontFamily = Mono) },
    )
}
