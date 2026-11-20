package dev.shed

import dev.shed.api.Cursor
import dev.shed.api.Screen
import dev.shed.api.ScreenLine
import dev.shed.api.ScreenRow
import dev.shed.api.Span
import dev.shed.ui.terminal.Rewrap
import dev.shed.ui.terminal.TerminalModel
import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalTest {
    private fun line(text: String, wraps: Boolean = false) = ScreenLine(listOf(Span(text)), wraps)
    private fun text(row: List<Span>) = row.joinToString("") { it.text }

    @Test
    fun wrappedRowsJoinAndRewrapToTheWidth() {
        // An 80-column screen: one long line over two rows, then a short one.
        val lines = listOf(line("a".repeat(80), wraps = true), line("b".repeat(20)), line("$ ls"))
        val rows = Rewrap.rewrap(lines, 40)
        assertEquals(listOf("a".repeat(40), "a".repeat(40), "b".repeat(20), "$ ls"), rows.map(::text))
    }

    @Test
    fun stylesSurviveTheCut() {
        val styled = ScreenLine(listOf(Span("red ", fg = "1"), Span("plain text")))
        val rows = Rewrap.rewrap(listOf(styled), 6)
        assertEquals(listOf("red pl", "ain te", "xt"), rows.map(::text))
        assertEquals("1", rows[0][0].fg)
        assertEquals(null, rows[0][1].fg)
    }

    @Test
    fun wideCharactersTakeTwoCells() {
        val rows = Rewrap.rewrap(listOf(line("日本語のテキスト")), 5)
        assertEquals(listOf("日本", "語の", "テキ", "スト"), rows.map(::text))
    }

    @Test
    fun trailingBlanksGoButEmptyLinesStay() {
        val rows = Rewrap.rewrap(listOf(line("hi      "), line(""), line("there")), 20)
        assertEquals(listOf("hi", "", "there"), rows.map(::text))
    }

    @Test
    fun modelAppliesFullAndPartialScreensAndKeepsScrolledLines() {
        val model = TerminalModel()
        model.apply(
            Screen(
                full = true, cols = 10u, rows = 3u,
                lines = listOf(ScreenRow(0u, line("one")), ScreenRow(1u, line("two")), ScreenRow(2u, line("three"))),
                cursor = Cursor(2u, 5u, false),
            ),
        )
        assertEquals(listOf("one", "two", "three"), model.screenLines.map { text(it.spans) })
        // One line scrolled off the top: the link sends the rows that changed.
        model.apply(
            Screen(
                full = false, cols = 10u, rows = 3u,
                lines = listOf(ScreenRow(0u, line("two")), ScreenRow(1u, line("three")), ScreenRow(2u, line("four"))),
                cursor = Cursor(2u, 4u, false), scrolled = 1u, scrollback = 1u,
            ),
        )
        assertEquals(listOf("one"), model.historyLines.map { text(it.spans) })
        assertEquals(listOf("two", "three", "four"), model.screenLines.map { text(it.spans) })
        assertEquals(0, model.moreHistory)
    }

    @Test
    fun cursorIsDrawnInverse() {
        val drawn = TerminalModel.withCursor(line("abc"), 1)
        assertEquals(listOf("a", "b", "c"), drawn.spans.map { it.text })
        assertEquals(true, drawn.spans[1].inverse)
        val past = TerminalModel.withCursor(line("ab"), 4)
        assertEquals("ab   ", text(past.spans))
        assertEquals(true, past.spans.last().inverse)
    }
}
