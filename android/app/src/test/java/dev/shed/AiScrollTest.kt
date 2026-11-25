package dev.shed

import dev.shed.api.AiAssistant
import dev.shed.api.AiEntry
import dev.shed.api.AiTool
import dev.shed.api.AiUser
import dev.shed.ui.ai.Follow
import dev.shed.ui.ai.atEnd
import dev.shed.ui.ai.nextPrompt
import dev.shed.ui.ai.previousPrompt
import dev.shed.ui.ai.promptIndices
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The conversation opening at its end, following new output, and the jumps between your messages. */
class AiScrollTest {
    @Test
    fun followsFromTheStart() {
        val f = Follow()
        assertTrue(f.following)
        // Output came: scroll down; nothing below: nothing to do.
        assertTrue(f.shouldScroll(moreBelow = true))
        assertFalse(f.shouldScroll(moreBelow = false))
        assertFalse(f.showJump(moreBelow = true))
    }

    @Test
    fun scrollingUpByHandStopsFollowing() {
        val f = Follow()
        f.dragged()
        // While the finger is on the list, new output does not pull it down.
        assertFalse(f.shouldScroll(moreBelow = true))
        f.settled(atEnd = false)
        assertFalse(f.following)
        assertFalse(f.shouldScroll(moreBelow = true))
        assertTrue(f.showJump(moreBelow = true))
    }

    @Test
    fun aDragThatEndsAtTheEndKeepsFollowing() {
        val f = Follow()
        f.dragged()
        f.settled(atEnd = true)
        assertTrue(f.following)
    }

    @Test
    fun scrollingBackDownStartsFollowingAgain() {
        val f = Follow(following = false)
        f.dragged()
        f.settled(atEnd = true)
        assertTrue(f.following)
        assertFalse(f.showJump(moreBelow = true))
    }

    @Test
    fun aScrollItDidItselfDoesNotStopFollowing() {
        // The list moving without a finger (scrolled to the end, content growing).
        val f = Follow()
        f.settled(atEnd = false)
        assertTrue(f.following)
    }

    @Test
    fun jumps() {
        val f = Follow()
        f.away()
        assertFalse(f.following)
        // A jump that lands at the end follows again.
        f.settled(atEnd = true)
        assertTrue(f.following)
        f.away()
        f.toLatest()
        assertTrue(f.following)
    }

    @Test
    fun theEnd() {
        assertTrue(atEnd(canScrollForward = false, lastVisible = 3, total = 10, lastItemEnd = 0, viewportEnd = 0, slack = 0))
        // The last item ends a hair below the viewport: at the end still.
        assertTrue(atEnd(canScrollForward = true, lastVisible = 9, total = 10, lastItemEnd = 1010, viewportEnd = 1000, slack = 24))
        assertFalse(atEnd(canScrollForward = true, lastVisible = 9, total = 10, lastItemEnd = 1100, viewportEnd = 1000, slack = 24))
        assertFalse(atEnd(canScrollForward = true, lastVisible = 8, total = 10, lastItemEnd = 1000, viewportEnd = 1000, slack = 24))
        assertFalse(atEnd(canScrollForward = true, lastVisible = null, total = 0, lastItemEnd = 0, viewportEnd = 0, slack = 24))
    }

    private fun user(text: String) = AiEntry.User(AiUser(text))
    private fun answer(text: String) = AiEntry.Assistant(AiAssistant(text))
    private fun tool() = AiEntry.Tool(AiTool("t", "Bash", "$ ls"))

    // 0 you, 1 answer, 2 tool, 3 answer, 4 you, 5 answer, 6 you, 7 tool
    private val entries = listOf(user("a"), answer("b"), tool(), answer("c"), user("d"), answer("e"), user("f"), tool())

    @Test
    fun yourMessages() {
        assertEquals(listOf(0, 4, 6), promptIndices(entries))
        assertEquals(emptyList<Int>(), promptIndices(listOf(answer("x"))))
    }

    @Test
    fun previousMessage() {
        // From an answer: the message it answers.
        assertEquals(4, previousPrompt(entries, top = 5, inside = false))
        // At the start of a message: the one before it.
        assertEquals(0, previousPrompt(entries, top = 4, inside = false))
        // Scrolled into a message: its own start first.
        assertEquals(4, previousPrompt(entries, top = 4, inside = true))
        assertNull(previousPrompt(entries, top = 0, inside = false))
        assertEquals(0, previousPrompt(entries, top = 0, inside = true))
    }

    @Test
    fun nextMessage() {
        assertEquals(4, nextPrompt(entries, top = 0))
        assertEquals(4, nextPrompt(entries, top = 2))
        assertEquals(6, nextPrompt(entries, top = 4))
        // Past the last: none (the jump goes to the end).
        assertNull(nextPrompt(entries, top = 6))
        assertNull(nextPrompt(entries, top = 7))
    }
}
