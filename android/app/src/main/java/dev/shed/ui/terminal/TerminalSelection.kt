package dev.shed.ui.terminal

import dev.shed.api.Span
import dev.shed.ui.common.cellWidth

// Selecting text in a terminal: by cells (wide characters take two), over
// rows as drawn (re-wrapped on the normal screen, as they are on the alt
// screen). Plain Kotlin, tested on the JVM.

/** A row as drawn: its spans, and whether the next row goes on with the same line. */
data class TermRow(val spans: List<Span>, val wraps: Boolean = false) {
    val text: String get() = spans.joinToString("") { it.text }
}

/** A cell: its row (in the rows drawn) and column. */
data class Cell(val row: Int, val col: Int) : Comparable<Cell> {
    override fun compareTo(other: Cell): Int = compareValuesBy(this, other, { it.row }, { it.col })
}

/** Cells from [anchor] to [focus], both in, whichever comes first. */
data class TermSelection(val anchor: Cell, val focus: Cell) {
    val first: Cell get() = minOf(anchor, focus)
    val last: Cell get() = maxOf(anchor, focus)

    /** The columns selected in [row] (to [Int.MAX_VALUE]: to its end), or null. */
    fun cols(row: Int): IntRange? {
        if (row < first.row || row > last.row) return null
        return (if (row == first.row) first.col else 0)..(if (row == last.row) last.col else Int.MAX_VALUE)
    }

    /** The same cells with rows added above (or taken away). */
    fun shifted(rows: Int) = TermSelection(anchor.copy(row = anchor.row + rows), focus.copy(row = focus.row + rows))
}

object Selecting {
    /** Where each cell's character is in [text] (a wide character's range for both its cells). */
    fun cells(text: String): List<IntRange> {
        val out = ArrayList<IntRange>()
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            val n = Character.charCount(cp)
            val range = i until i + n
            repeat(cellWidth(cp).coerceAtLeast(1)) { out.add(range) }
            i += n
        }
        return out
    }

    /** The text in cells [from] to [to] (both in) of [text], clamped to what is there. */
    fun slice(text: String, from: Int, to: Int): String {
        val cells = cells(text)
        val a = from.coerceAtLeast(0)
        if (a >= cells.size || to < a) return ""
        val b = to.coerceAtMost(cells.size - 1)
        return text.substring(cells[a].first, cells[b].last + 1)
    }

    /** What ends a word: blanks, quotes, brackets and separators (paths and addresses stay whole). */
    private const val BREAKS = "\"'`()[]{}<>,;|"

    private fun isWord(s: String) = s.isNotBlank() && s.none { it in BREAKS }

    /** The word at [at] (a long press there); on a blank or past the end, that cell alone. */
    fun wordAt(rows: List<TermRow>, at: Cell): TermSelection {
        val row = rows.getOrNull(at.row) ?: return TermSelection(at, at)
        val text = row.text
        val cells = cells(text)
        fun word(c: Int) = isWord(text.substring(cells[c].first, cells[c].last + 1))
        if (at.col !in cells.indices || !word(at.col)) return TermSelection(at, at)
        var a = at.col
        while (a > 0 && word(a - 1)) a--
        var b = at.col
        while (b < cells.size - 1 && word(b + 1)) b++
        return TermSelection(Cell(at.row, a), Cell(at.row, b))
    }

    /** The whole line [at] is on, over the rows it wraps into. */
    fun lineAt(rows: List<TermRow>, at: Cell): TermSelection {
        if (rows.isEmpty()) return TermSelection(at, at)
        var a = at.row.coerceIn(0, rows.size - 1)
        while (a > 0 && rows[a - 1].wraps) a--
        var b = at.row.coerceIn(0, rows.size - 1)
        while (b < rows.size - 1 && rows[b].wraps) b++
        return TermSelection(Cell(a, 0), Cell(b, (cells(rows[b].text).size - 1).coerceAtLeast(0)))
    }

    /** [sel] grown to whole lines. */
    fun lines(rows: List<TermRow>, sel: TermSelection): TermSelection =
        TermSelection(lineAt(rows, sel.first).first, lineAt(rows, sel.last).last)

    /** Everything. */
    fun all(rows: List<TermRow>): TermSelection =
        TermSelection(Cell(0, 0), Cell((rows.size - 1).coerceAtLeast(0), (cells(rows.lastOrNull()?.text.orEmpty()).size - 1).coerceAtLeast(0)))

    /**
     * The selected text: rows that wrap join without a break (the program's
     * own line comes back whole), lines end without their trailing blanks.
     */
    fun text(rows: List<TermRow>, sel: TermSelection): String {
        val out = StringBuilder()
        fun trimLine() {
            while (out.isNotEmpty() && out.last() == ' ') out.setLength(out.length - 1)
        }
        for (r in sel.first.row..sel.last.row) {
            val row = rows.getOrNull(r) ?: continue
            val cols = sel.cols(r) ?: continue
            out.append(slice(row.text, cols.first, cols.last))
            if (r < sel.last.row && !row.wraps) {
                trimLine()
                out.append('\n')
            }
        }
        trimLine()
        return out.toString()
    }
}
