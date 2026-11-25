package dev.shed

import dev.shed.api.ScreenLine
import dev.shed.api.Span
import dev.shed.ui.terminal.Cell
import dev.shed.ui.terminal.Rewrap
import dev.shed.ui.terminal.Selecting
import dev.shed.ui.terminal.TermRow
import dev.shed.ui.terminal.TermSelection
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Selecting text in a terminal: words, lines, wrapped rows, wide characters. */
class TerminalSelectionTest {
    private fun row(text: String, wraps: Boolean = false) = TermRow(listOf(Span(text)), wraps)
    private fun sel(r0: Int, c0: Int, r1: Int, c1: Int) = TermSelection(Cell(r0, c0), Cell(r1, c1))

    private val rows = listOf(
        row("you@shed:~/shed$ cat \"app/src/main.rs\" | wc -l"),
        row("     42"),
        row("you@shed:~/shed$ "),
    )

    @Test
    fun aLongPressSelectsTheWordThere() {
        // A path stays whole; the quotes around it do not come along.
        assertEquals(sel(0, 22, 0, 36), Selecting.wordAt(rows, Cell(0, 30)))
        assertEquals("app/src/main.rs", Selecting.text(rows, Selecting.wordAt(rows, Cell(0, 30))))
        assertEquals("you@shed:~/shed$", Selecting.text(rows, Selecting.wordAt(rows, Cell(0, 3))))
        assertEquals("42", Selecting.text(rows, Selecting.wordAt(rows, Cell(1, 6))))
        // On a blank, or past the end: that cell alone.
        assertEquals(sel(1, 2, 1, 2), Selecting.wordAt(rows, Cell(1, 2)))
        assertEquals(sel(1, 30, 1, 30), Selecting.wordAt(rows, Cell(1, 30)))
        assertEquals(sel(9, 0, 9, 0), Selecting.wordAt(rows, Cell(9, 0)))
    }

    @Test
    fun selectionsReadTheSameEitherWay() {
        val forward = sel(0, 17, 1, 6)
        val backward = TermSelection(forward.focus, forward.anchor)
        assertEquals("cat \"app/src/main.rs\" | wc -l\n     42", Selecting.text(rows, forward))
        assertEquals(Selecting.text(rows, forward), Selecting.text(rows, backward))
    }

    @Test
    fun rowsOfOneLineJoinWithoutABreak() {
        val lines = listOf(ScreenLine(listOf(Span("0123456789abcdef")), wraps = false), ScreenLine(listOf(Span("next   ")), wraps = false))
        val drawn = Rewrap.rows(lines, 6)
        assertEquals(listOf(true, true, false, false), drawn.map { it.wraps })
        assertEquals(listOf("012345", "6789ab", "cdef", "next"), drawn.map { it.text })
        // From the middle of the first row to the second line: one line, then the next.
        assertEquals("456789abcdef\nnext", Selecting.text(drawn, sel(0, 4, 3, 99)))
        // A long press's line covers every row it wraps into.
        assertEquals(sel(0, 0, 2, 3), Selecting.lineAt(drawn, Cell(1, 2)))
        assertEquals(sel(0, 0, 3, 3), Selecting.lines(drawn, sel(1, 2, 3, 1)))
        assertEquals("0123456789abcdef\nnext", Selecting.text(drawn, Selecting.all(drawn)))
    }

    @Test
    fun trailingBlanksAreLeftOut() {
        val screen = listOf(row("ls      "), row("a  b    "))
        assertEquals("ls\na  b", Selecting.text(screen, sel(0, 0, 1, 7)))
    }

    @Test
    fun wideCharactersTakeTwoCells() {
        val wide = listOf(row("日本 ok"))
        assertEquals(7, Selecting.cells("日本 ok").size)
        // Either cell of a wide character is that character.
        assertEquals("本", Selecting.slice("日本 ok", 3, 3))
        assertEquals("日本", Selecting.text(wide, Selecting.wordAt(wide, Cell(0, 1))))
        assertEquals("ok", Selecting.text(wide, Selecting.wordAt(wide, Cell(0, 6))))
        assertEquals("", Selecting.slice("ab", 5, 9))
    }

    @Test
    fun selectedColumnsPerRow() {
        val s = sel(2, 5, 4, 1)
        assertNull(s.cols(1))
        assertEquals(5..Int.MAX_VALUE, s.cols(2))
        assertEquals(0..Int.MAX_VALUE, s.cols(3))
        assertEquals(0..1, s.cols(4))
        assertNull(s.cols(5))
        assertEquals(sel(5, 5, 7, 1), s.shifted(3))
    }
}
