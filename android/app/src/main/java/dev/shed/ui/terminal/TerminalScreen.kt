package dev.shed.ui.terminal

import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.automirrored.filled.Notes
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.SelectAll
import androidx.compose.material3.TextButton
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.platform.LocalHapticFeedback
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.ListScrollbar
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.logbook.copy
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.WindowInsets
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
        // The key bar pads the navigation bar itself (in its colour); with the
        // keyboard up this pads all of it, and the key bar's padding comes to
        // nothing (consumed), so nothing is counted twice or left out.
        Column(
            Modifier
                .fillMaxSize()
                .padding(top = padding.calculateTopPadding())
                .windowInsetsPadding(WindowInsets.ime),
        ) {
            val onMouse: ((List<Mouse>) -> Unit)? = if (canChange) { m -> send(m.map { InputItem.Mouse(it) }) } else null
            TerminalView(vm, Modifier.weight(1f), takeSize, onMouse) { cols, rows ->
                runner.run { session.socket.call(ws, Call.TakeSize(TakeSize(pane.id, cols.toUInt(), rows.toUInt()))) }
            }
            if (canChange) InputBar(::send) else Spacer(Modifier.navigationBarsPadding())
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
    val context = LocalContext.current
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    val copied = stringResource(R.string.copied)
    val fontSize = 13.sp
    val style = TextStyle(fontFamily = Mono, fontSize = fontSize, lineHeight = 16.sp, color = shed.terminalForeground)
    val cell = remember(style) { measurer.measure("MMMMMMMMMM", style).size }
    val charW = cell.width / 10f
    val lineH = with(density) { 16.sp.toPx() }
    val model = vm.model
    var selection by remember { mutableStateOf<TermSelection?>(null) }
    val normalList = rememberLazyListState()
    Surface(modifier.fillMaxWidth(), color = shed.terminalBackground) {
        BoxWithConstraints(Modifier.fillMaxSize().padding(horizontal = 4.dp)) {
            val widthPx = with(density) { maxWidth.toPx() }
            val heightPx = with(density) { maxHeight.toPx() }
            val cols = (widthPx / charW).toInt().coerceAtLeast(10)
            val rows = (heightPx / lineH).toInt().coerceAtLeast(4)
            LaunchedEffect(takeSize, cols, rows) { if (takeSize) onSize(cols, rows) }
            // Another screen, another width: what was selected is gone.
            LaunchedEffect(model.alt, cols) { selection = null }
            if (version == 0) {
                EmptyNote(stringResource(if (ended) R.string.terminal_ended else R.string.terminal_waiting))
                return@BoxWithConstraints
            }
            val drawn = if (model.alt) {
                remember(version) { model.visibleScreen().map { TermRow(it.spans, it.wraps == true) } }
            } else {
                remember(version, cols) { Rewrap.rows(model.historyLines + model.visibleScreen(), cols) }
            }
            if (model.alt) {
                AltScreen(model, drawn, style, widthPx, charW, lineH, shed, onMouse, selection) { selection = it }
            } else {
                NormalScreen(vm, normalList, version, drawn, cols, style, charW, shed, selection) { selection = it }
            }
            selection?.let { sel ->
                // Away from what is selected: at the bottom when it is in the upper half of what shows.
                val upper = if (model.alt) {
                    sel.first.row < model.rows / 2
                } else {
                    val seen = normalList.layoutInfo.visibleItemsInfo
                    seen.isNotEmpty() && sel.first.row < seen.first().index + seen.size / 2
                }
                SelectionBar(
                    if (upper) Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp) else Modifier.align(Alignment.TopCenter).padding(top = 6.dp),
                    onCopy = {
                        copy(context, Selecting.text(drawn, sel))
                        selection = null
                        scope.launch { snackbar.showSnackbar(copied) }
                    },
                    onLines = { selection = Selecting.lines(drawn, sel) },
                    onAll = { selection = Selecting.all(drawn) },
                    onClose = { selection = null },
                )
            }
        }
    }
}

/** What to do with a selection: Copy, grow it to whole lines, select everything, or let it go. */
@Composable
private fun SelectionBar(modifier: Modifier, onCopy: () -> Unit, onLines: () -> Unit, onAll: () -> Unit, onClose: () -> Unit) {
    val shed = LocalShed.current
    Surface(
        modifier,
        shape = RoundedCornerShape(20.dp),
        color = shed.surface,
        border = BorderStroke(1.dp, shed.divider),
        shadowElevation = 4.dp,
    ) {
        Row(Modifier.padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onCopy) { Icon(Icons.Default.ContentCopy, null, Modifier.size(18.dp)); HSpace(6); Text(stringResource(R.string.copy)) }
            TextButton(onClick = onLines) { Icon(Icons.AutoMirrored.Filled.Notes, null, Modifier.size(18.dp)); HSpace(6); Text(stringResource(R.string.terminal_select_lines)) }
            TextButton(onClick = onAll) { Icon(Icons.Default.SelectAll, null, Modifier.size(18.dp)); HSpace(6); Text(stringResource(R.string.terminal_select_all)) }
            IconButton(onClick = onClose) { Icon(Icons.Default.Close, stringResource(R.string.terminal_select_done), Modifier.size(18.dp)) }
        }
    }
}

/** Where cells are on a screen as drawn, for selecting. */
private interface CellGrid {
    val cellW: Float
    val cellH: Float

    /** The cell under a point (the nearest row in view). */
    fun cellAt(p: Offset): Cell?

    /** A cell's top-left corner, or null when it is out of view. */
    fun topLeft(c: Cell): Offset?
}

/**
 * Selecting on a screen: a long press selects the word there, a tap lets
 * the selection go, the handles under its ends drag them. Handles are taken
 * before the list scrolls; everything else goes on as before.
 */
@Composable
private fun Modifier.selecting(
    grid: CellGrid,
    rows: List<TermRow>,
    selection: TermSelection?,
    onSelect: (TermSelection?) -> Unit,
): Modifier {
    val shed = LocalShed.current
    val haptics = LocalHapticFeedback.current
    val density = LocalDensity.current
    val radius = with(density) { 9.dp.toPx() }
    val reach = with(density) { 28.dp.toPx() }
    val stem = with(density) { 2.dp.toPx() }
    val g by rememberUpdatedState(grid)
    val r by rememberUpdatedState(rows)
    val sel by rememberUpdatedState(selection)
    val set by rememberUpdatedState(onSelect)
    // The ends' handles: under the first cell's left edge and the last cell's right edge.
    fun handles(): Pair<Offset?, Offset?> {
        val s = sel ?: return null to null
        val a = g.topLeft(s.first)?.let { Offset(it.x, it.y + g.cellH) }
        val b = g.topLeft(s.last)?.let { Offset(it.x + g.cellW, it.y + g.cellH) }
        return a to b
    }
    return this
        .pointerInput(Unit) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                val s = sel ?: return@awaitEachGesture
                val (a, b) = handles()
                fun near(h: Offset?) = h != null && (down.position - Offset(h.x, h.y + radius)).getDistance() < reach
                val start = near(a)
                if (!start && !near(b)) return@awaitEachGesture
                down.consume()
                // The other end stays; this one follows the finger (aiming at the row above the handle).
                val fixed = if (start) s.last else s.first
                val nudge = if (start) g.cellW * 0.3f else -g.cellW * 0.7f
                while (true) {
                    val event = awaitPointerEvent(PointerEventPass.Initial)
                    val change = event.changes.firstOrNull { it.id == down.id } ?: break
                    change.consume()
                    if (!change.pressed) break
                    val at = g.cellAt(Offset(change.position.x + nudge, change.position.y - radius - g.cellH / 2)) ?: continue
                    set(TermSelection(fixed, at))
                }
            }
        }
        .pointerInput(Unit) {
            detectTapGestures(
                onLongPress = { p ->
                    val at = g.cellAt(p) ?: return@detectTapGestures
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    set(Selecting.wordAt(r, at))
                },
                onTap = { if (sel != null) set(null) },
            )
        }
        .drawWithContent {
            drawContent()
            val (a, b) = handles()
            for (h in listOfNotNull(a, b)) {
                drawRect(shed.accent, topLeft = Offset(h.x - stem / 2, h.y - g.cellH), size = Size(stem, g.cellH))
                drawCircle(shed.accent, radius, Offset(h.x, h.y + radius))
            }
        }
}

/** The selected part of a row, drawn behind its text. */
private fun Modifier.selected(selection: TermSelection?, row: Int, cellW: Float, color: Color): Modifier {
    val cols = selection?.cols(row) ?: return this
    return drawBehind {
        val x0 = cols.first * cellW
        val x1 = if (cols.last == Int.MAX_VALUE) size.width.coerceAtLeast(x0 + cellW) else (cols.last + 1) * cellW
        drawRect(color, topLeft = Offset(x0, 0f), size = Size(x1 - x0, size.height))
    }
}

@Composable
private fun NormalScreen(
    vm: TerminalVm,
    list: LazyListState,
    version: Int,
    drawn: List<TermRow>,
    cols: Int,
    style: TextStyle,
    charW: Float,
    shed: ShedColors,
    selection: TermSelection?,
    onSelect: (TermSelection?) -> Unit,
) {
    val model = vm.model
    val rows = remember(drawn, shed) { drawn.map { styled(it.spans, shed) } }
    var follow by remember { mutableStateOf(true) }
    val scope = rememberCoroutineScope()
    val currentSelection by rememberUpdatedState(selection)
    LaunchedEffect(list.isScrollInProgress) {
        if (!list.isScrollInProgress) {
            val last = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            follow = last >= list.layoutInfo.totalItemsCount - 2
        }
    }
    // New output keeps the end in view, unless something is selected (it would scroll away).
    LaunchedEffect(rows.size, version) {
        if (follow && selection == null && rows.isNotEmpty()) list.scrollToItem(rows.size - 1)
    }
    val atTop by remember { derivedStateOf { list.firstVisibleItemIndex == 0 && !follow } }
    LaunchedEffect(atTop) {
        if (atTop) {
            val before = rows.size
            vm.loadMore { _ ->
                scope.launch {
                    // Keep what was at the top where it was (and what is selected on its text).
                    kotlinx.coroutines.delay(16)
                    val added = (Rewrap.rewrap(model.historyLines + model.visibleScreen(), cols).size - before).coerceAtLeast(0)
                    currentSelection?.let { if (added > 0) onSelect(it.shifted(added)) }
                    list.scrollToItem(added)
                }
            }
        }
    }
    val lineH = with(LocalDensity.current) { style.lineHeight.toPx() }
    val grid = remember(list, charW, lineH) {
        object : CellGrid {
            override val cellW = charW
            override val cellH = lineH
            override fun cellAt(p: Offset): Cell? {
                val items = list.layoutInfo.visibleItemsInfo
                if (items.isEmpty()) return null
                val item = items.firstOrNull { p.y >= it.offset && p.y < it.offset + it.size }
                    ?: if (p.y < items.first().offset) items.first() else items.last()
                return Cell(item.index, (p.x / charW).toInt().coerceAtLeast(0))
            }
            override fun topLeft(c: Cell): Offset? =
                list.layoutInfo.visibleItemsInfo.firstOrNull { it.index == c.row }?.let { Offset(c.col * charW, it.offset.toFloat()) }
        }
    }
    val highlight = shed.accent.copy(alpha = 0.35f)
    Box(Modifier.fillMaxSize().selecting(grid, drawn, selection, onSelect)) {
        LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
            itemsIndexed(rows) { i, row ->
                Text(row, style = style, softWrap = false, maxLines = 1, modifier = Modifier.fillMaxWidth().selected(selection, i, charW, highlight))
            }
        }
        ListScrollbar(list, Modifier.align(Alignment.TopEnd)) { follow = false }
    }
}

@Composable
private fun AltScreen(
    model: TerminalModel,
    drawn: List<TermRow>,
    style: TextStyle,
    widthPx: Float,
    charW: Float,
    lineH: Float,
    shed: ShedColors,
    onMouse: ((List<Mouse>) -> Unit)?,
    selection: TermSelection?,
    onSelect: (TermSelection?) -> Unit,
) {
    var fit by rememberSaveable { mutableStateOf(true) }
    val scale = if (fit) (widthPx / (model.cols * charW)).coerceIn(0.4f, 1f) else 1f
    val small = style.copy(fontSize = style.fontSize * scale, lineHeight = style.lineHeight * scale)
    val lines = remember(drawn, shed) { drawn.map { styled(it.spans, shed) } }
    val cellW = charW * scale
    val cellH = lineH * scale
    val haptics = LocalHapticFeedback.current
    val rowsNow by rememberUpdatedState(drawn)
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            FilterChip(
                selected = fit,
                onClick = { fit = !fit },
                label = { Text(stringResource(R.string.terminal_fit), fontSize = 11.sp) },
            )
        }
        val scroll = rememberScrollState()
        val grid = remember(cellW, cellH, model.rows) {
            object : CellGrid {
                override val cellW = cellW
                override val cellH = cellH
                override fun cellAt(p: Offset) = Cell((p.y / cellH).toInt().coerceIn(0, (model.rows - 1).coerceAtLeast(0)), (p.x / cellW).toInt().coerceAtLeast(0))
                override fun topLeft(c: Cell) = Offset(c.col * cellW, c.row * cellH)
            }
        }
        // The program has the mouse: a tap clicks; a long press still selects.
        val mouse = if (onMouse != null && model.mouse) {
            Modifier.mouseInput(model, cellW, cellH, onMouse) { at ->
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                onSelect(Selecting.wordAt(rowsNow, grid.cellAt(at) ?: return@mouseInput))
            }
        } else {
            Modifier
        }
        val highlight = shed.accent.copy(alpha = 0.35f)
        Column(
            Modifier
                .fillMaxSize()
                .then(if (fit) Modifier else Modifier.horizontalScroll(scroll))
                .selecting(grid, drawn, selection, onSelect)
                .then(mouse),
        ) {
            lines.forEachIndexed { i, line ->
                Text(line, style = small, softWrap = false, maxLines = 1, modifier = Modifier.selected(selection, i, cellW, highlight))
            }
        }
    }
}

/**
 * The program asked for the mouse (`Screen.mouse`): a tap is a left click at
 * its cell, a vertical drag turns the wheel a notch per row dragged (finger
 * down: wheel up, as content follows the finger); a long press is [onLongPress].
 */
@Composable
private fun Modifier.mouseInput(model: TerminalModel, cellW: Float, cellH: Float, onMouse: (List<Mouse>) -> Unit, onLongPress: (Offset) -> Unit): Modifier {
    val send by rememberUpdatedState(onMouse)
    val longPress by rememberUpdatedState(onLongPress)
    fun cell(at: Offset): Pair<UInt, UInt> {
        val col = (at.x / cellW).toInt().coerceIn(0, model.cols - 1)
        val row = (at.y / cellH).toInt().coerceIn(0, model.rows - 1)
        return col.toUInt() to row.toUInt()
    }
    return pointerInput(cellW, cellH) {
        detectTapGestures(onLongPress = { longPress(it) }) { at ->
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
