package dev.shed.ui.common

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.shed.ui.theme.LocalShed
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Where a lazy list is, for a scrollbar: estimated from the items in view,
 * whose mean size stands in for the ones not laid out (items of very
 * different heights make it move a little unevenly, never past the ends).
 * Plain Kotlin, tested on the JVM.
 */
object ScrollbarMath {
    /** An item in view: its index, its start (from the content's start, after the padding) and size, in pixels. */
    data class Seen(val index: Int, val offset: Int, val size: Int)

    /** A list as laid out now. */
    data class Layout(
        val seen: List<Seen>,
        val total: Int,
        /** The viewport's length, paddings included. */
        val viewport: Int,
        val spacing: Int = 0,
        /** The content padding, before and after together. */
        val padding: Int = 0,
        val atStart: Boolean = false,
        val atEnd: Boolean = false,
    )

    /** The thumb's top and length along the track. */
    data class Thumb(val top: Float, val length: Float)

    /** The mean distance from one item's start to the next one's. */
    fun stride(l: Layout): Float =
        if (l.seen.isEmpty()) 0f else l.seen.sumOf { it.size }.toFloat() / l.seen.size + l.spacing

    /** The whole content's estimated length. */
    fun content(l: Layout): Float = (stride(l) * l.total - l.spacing + l.padding).coerceAtLeast(0f)

    /** How far down the list is, 0 (top) to 1 (end). */
    fun fraction(l: Layout): Float {
        if (l.atStart || l.seen.isEmpty()) return 0f
        if (l.atEnd) return 1f
        val first = l.seen.minBy { it.index }
        val scrolled = first.index * stride(l) - first.offset
        val max = content(l) - l.viewport
        return if (max <= 0f) 0f else (scrolled / max).coerceIn(0f, 1f)
    }

    /** The thumb on a [track] pixels long, at least [minLength]; `null`: it all fits, no thumb. */
    fun thumb(l: Layout, track: Float, minLength: Float): Thumb? {
        if (l.seen.isEmpty() || l.total == 0 || (l.atStart && l.atEnd) || track <= 0f) return null
        val content = content(l).coerceAtLeast(l.viewport + 1f)
        val length = (track * l.viewport / content).coerceIn(minLength.coerceAtMost(track), track)
        return Thumb((track - length) * fraction(l), length)
    }

    /** The thumb's top on the track as a fraction of the way it can go. */
    fun dragFraction(top: Float, length: Float, track: Float): Float =
        if (track <= length) 0f else (top / (track - length)).coerceIn(0f, 1f)

    /** Where to scroll for [fraction] of the way: an item, and pixels into it. */
    fun target(l: Layout, fraction: Float): Pair<Int, Int> {
        val stride = stride(l)
        if (l.total == 0 || stride <= 0f) return 0 to 0
        val px = fraction.coerceIn(0f, 1f) * (content(l) - l.viewport).coerceAtLeast(0f)
        val index = (px / stride).toInt().coerceIn(0, l.total - 1)
        val into = (px - index * stride).toInt().coerceAtLeast(0)
        return index to into
    }
}

/** [state] as [ScrollbarMath] sees it. */
fun LazyListState.scrollbarLayout(): ScrollbarMath.Layout {
    val info = layoutInfo
    return ScrollbarMath.Layout(
        seen = info.visibleItemsInfo.map { ScrollbarMath.Seen(it.index, it.offset, it.size) },
        total = info.totalItemsCount,
        viewport = info.viewportSize.height,
        spacing = info.mainAxisItemSpacing,
        padding = info.beforeContentPadding + info.afterContentPadding,
        atStart = !canScrollBackward,
        atEnd = !canScrollForward,
    )
}

/**
 * A thin scrollbar over a lazy list's right edge: it shows while you scroll
 * the list and fades a second after; while shown, dragging along the edge
 * moves through the list quickly (the thumb follows the finger). [onDrag]:
 * the scrollbar moved the list. Put it in a `Box` over the list, aligned to its end.
 */
@Composable
fun ListScrollbar(state: LazyListState, modifier: Modifier = Modifier, onDrag: () -> Unit = {}) {
    val shed = LocalShed.current
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val minThumb = with(density) { 36.dp.toPx() }
    val thin = with(density) { 4.dp.toPx() }
    val wide = with(density) { 6.dp.toPx() }
    val edge = with(density) { 2.dp.toPx() }
    val dragged by rememberUpdatedState(onDrag)
    val alpha = remember { Animatable(0f) }
    var dragging by remember { mutableStateOf(false) }
    var dragTop by remember { mutableFloatStateOf(0f) }
    var dragLength by remember { mutableFloatStateOf(0f) }
    // Shown for your own scrolling (and its fling), not for the list following new output.
    val fingerOnList by state.interactionSource.collectIsDraggedAsState()
    var byHand by remember { mutableStateOf(false) }
    if (fingerOnList) byHand = true
    LaunchedEffect(state.isScrollInProgress, fingerOnList) { if (!state.isScrollInProgress && !fingerOnList) byHand = false }
    val active = dragging || fingerOnList || (byHand && state.isScrollInProgress)
    LaunchedEffect(active) {
        if (active) {
            alpha.animateTo(1f, tween(120))
        } else {
            delay(1000)
            alpha.animateTo(0f, tween(300))
        }
    }
    val shown = alpha.value > 0f
    Box(
        modifier
            .fillMaxHeight()
            .width(if (shown) 28.dp else 8.dp)
            .semantics { hideFromAccessibility() }
            .drawBehind {
                val a = alpha.value
                if (a <= 0f) return@drawBehind
                val thumb = if (dragging) {
                    ScrollbarMath.Thumb(dragTop, dragLength)
                } else {
                    ScrollbarMath.thumb(state.scrollbarLayout(), size.height, minThumb) ?: return@drawBehind
                }
                val w = if (dragging) wide else thin
                drawRoundRect(
                    color = (if (dragging) shed.accent else shed.textDim).copy(alpha = 0.7f * a),
                    topLeft = Offset(size.width - w - edge, thumb.top),
                    size = Size(w, thumb.length),
                    cornerRadius = CornerRadius(w / 2),
                )
            }
            .then(
                if (!shown) {
                    Modifier
                } else {
                    Modifier.pointerInput(state) {
                        var grab = 0f
                        fun moveTo(y: Float) {
                            val track = size.height.toFloat()
                            dragTop = (y - grab).coerceIn(0f, (track - dragLength).coerceAtLeast(0f))
                            val (index, into) = ScrollbarMath.target(state.scrollbarLayout(), ScrollbarMath.dragFraction(dragTop, dragLength, track))
                            dragged()
                            scope.launch { state.scrollToItem(index, into) }
                        }
                        detectVerticalDragGestures(
                            onDragStart = { at ->
                                val track = size.height.toFloat()
                                val thumb = ScrollbarMath.thumb(state.scrollbarLayout(), track, minThumb) ?: return@detectVerticalDragGestures
                                dragLength = thumb.length
                                dragTop = thumb.top
                                // Held by the thumb: it keeps where the finger took it; elsewhere it comes to the finger.
                                grab = if (at.y in thumb.top..(thumb.top + thumb.length)) at.y - thumb.top else thumb.length / 2
                                dragging = true
                                moveTo(at.y)
                            },
                            onDragEnd = { dragging = false },
                            onDragCancel = { dragging = false },
                        ) { change, _ ->
                            if (!dragging) return@detectVerticalDragGestures
                            change.consume()
                            moveTo(change.position.y)
                        }
                    }
                },
            ),
    )
}
