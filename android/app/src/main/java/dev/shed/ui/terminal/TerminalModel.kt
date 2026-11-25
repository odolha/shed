package dev.shed.ui.terminal

import dev.shed.api.Cursor
import dev.shed.api.Screen
import dev.shed.api.ScreenLine
import dev.shed.api.Span
import dev.shed.ui.common.cellWidth

/**
 * A terminal pane's screen as the link sends it (`Screen` events): the rows,
 * the cursor, and the scrollback fetched so far. Plain Kotlin, no Android.
 */
class TerminalModel {
    var cols = 80
        private set
    var rows = 24
        private set
    var alt = false
        private set
    var cursor = Cursor(0u, 0u, false)
        private set
    var title: String? = null
        private set
    var mouse = false
        private set

    /** How many lines of scrollback the pane has in all. */
    var scrollbackTotal = 0
        private set

    /** Bumped on every change, for views to notice. */
    var version = 0
        private set

    private var screen = MutableList(rows) { BLANK }

    /**
     * Scrollback held here, oldest first, ending just above the screen: lines
     * fetched with `Scrollback` and lines seen scrolling off the top.
     */
    private val history = ArrayDeque<ScreenLine>()

    val screenLines: List<ScreenLine> get() = screen
    val historyLines: List<ScreenLine> get() = history

    /** How many more lines of scrollback there are to fetch. */
    val moreHistory: Int get() = (scrollbackTotal - history.size).coerceAtLeast(0)

    fun apply(event: Screen) {
        val scrolled = (event.scrolled ?: 0u).toInt()
        val wasAlt = alt
        alt = event.alt == true
        // Rows that scrolled off the top go into scrollback, so a view keeps its place.
        if (scrolled > 0 && !wasAlt && !alt) {
            for (i in 0 until scrolled.coerceAtMost(screen.size)) history.addLast(screen[i])
            while (history.size > MAX_HISTORY) history.removeFirst()
        }
        val newCols = event.cols.toInt().coerceAtLeast(1)
        val newRows = event.rows.toInt().coerceAtLeast(1)
        if (event.full) {
            screen = MutableList(newRows) { BLANK }
        } else if (newRows != rows) {
            screen = MutableList(newRows) { screen.getOrElse(it) { BLANK } }
        }
        cols = newCols
        rows = newRows
        for (row in event.lines) {
            val r = row.row.toInt()
            if (r in 0 until rows) screen[r] = row.line
        }
        cursor = event.cursor
        title = event.title
        mouse = event.mouse == true
        scrollbackTotal = (event.scrollback ?: 0u).toInt()
        version++
    }

    /** Older lines fetched with `Scrollback` (oldest first), going above what is held. */
    fun prependHistory(lines: List<ScreenLine>) {
        for (line in lines.asReversed()) history.addFirst(line)
        while (history.size > MAX_HISTORY) history.removeLast()
        version++
    }

    /** Forgets the scrollback held (a new subscription starts afresh). */
    fun clearHistory() {
        history.clear()
        version++
    }

    /**
     * The screen's rows with the cursor drawn in (an inverse cell), and on the
     * normal screen with trailing blank rows below the cursor left out.
     */
    fun visibleScreen(): List<ScreenLine> {
        val lines = screen.toMutableList()
        val row = cursor.row.toInt()
        if (cursor.visible && row in lines.indices) lines[row] = withCursor(lines[row], cursor.col.toInt())
        if (!alt) {
            var last = lines.size - 1
            while (last > row.coerceAtLeast(0) && isBlank(lines[last])) last--
            return lines.subList(0, (last + 1).coerceAtMost(lines.size))
        }
        return lines
    }

    companion object {
        const val MAX_HISTORY = 5000
        val BLANK = ScreenLine(emptyList())

        fun isBlank(line: ScreenLine) = line.spans.all { it.text.isBlank() && it.bg == null }

        /** [line] with the cell at [col] drawn inverse (padded with spaces if the line is shorter). */
        fun withCursor(line: ScreenLine, col: Int): ScreenLine {
            val out = ArrayList<Span>()
            var cells = 0
            var done = false
            for (span in line.spans) {
                if (done) {
                    out.add(span)
                    continue
                }
                val text = span.text
                var i = 0
                var start = -1
                while (i < text.length) {
                    val cp = text.codePointAt(i)
                    val w = cellWidth(cp)
                    if (col >= cells && col < cells + w) {
                        start = i
                        break
                    }
                    cells += w
                    i += Character.charCount(cp)
                }
                if (start < 0) {
                    out.add(span)
                    continue
                }
                val end = start + Character.charCount(text.codePointAt(start))
                if (start > 0) out.add(span.copy(text = text.substring(0, start)))
                out.add(span.copy(text = text.substring(start, end), inverse = span.inverse != true))
                if (end < text.length) out.add(span.copy(text = text.substring(end)))
                done = true
            }
            if (!done) {
                val pad = (col - cells).coerceAtLeast(0)
                if (pad > 0) out.add(Span(" ".repeat(pad)))
                out.add(Span(" ", inverse = true))
            }
            return line.copy(spans = out)
        }
    }
}

/** Re-wrapping a normal screen's lines to a narrower (or wider) width. */
object Rewrap {
    /** Rows joined where they wrap into the next: the program's own lines. */
    fun logical(lines: List<ScreenLine>): List<List<Span>> {
        val out = ArrayList<List<Span>>()
        var current = ArrayList<Span>()
        for (line in lines) {
            current.addAll(line.spans)
            if (line.wraps != true) {
                out.add(current)
                current = ArrayList()
            }
        }
        if (current.isNotEmpty()) out.add(current)
        return out
    }

    /** Trailing blanks without a background left out. */
    fun trimEnd(spans: List<Span>): List<Span> {
        val out = spans.toMutableList()
        while (out.isNotEmpty()) {
            val last = out.last()
            if (last.bg != null || last.inverse == true) break
            val trimmed = last.text.trimEnd()
            if (trimmed.isEmpty()) {
                out.removeAt(out.size - 1)
            } else {
                out[out.size - 1] = last.copy(text = trimmed)
                break
            }
        }
        return out
    }

    /** One logical line cut into rows of at most [width] cells (wide characters take two). */
    fun wrap(spans: List<Span>, width: Int): List<List<Span>> {
        val w = width.coerceAtLeast(1)
        val rows = ArrayList<List<Span>>()
        var row = ArrayList<Span>()
        var cells = 0
        for (span in spans) {
            val text = span.text
            var start = 0
            var i = 0
            while (i < text.length) {
                val cp = text.codePointAt(i)
                val cw = cellWidth(cp)
                if (cells + cw > w && cells > 0) {
                    if (i > start) row.add(span.copy(text = text.substring(start, i)))
                    rows.add(row)
                    row = ArrayList()
                    cells = 0
                    start = i
                }
                cells += cw
                i += Character.charCount(cp)
            }
            if (i > start) row.add(span.copy(text = text.substring(start, i)))
        }
        rows.add(row)
        return rows
    }

    /** Lines as a phone [width] cells wide shows them. */
    fun rewrap(lines: List<ScreenLine>, width: Int): List<List<Span>> = rows(lines, width).map { it.spans }

    /** Lines as a phone [width] cells wide shows them, each row saying whether the next goes on with its line. */
    fun rows(lines: List<ScreenLine>, width: Int): List<TermRow> = logical(lines).flatMap { line ->
        val parts = wrap(trimEnd(line), width)
        parts.mapIndexed { i, part -> TermRow(part, wraps = i < parts.size - 1) }
    }
}
