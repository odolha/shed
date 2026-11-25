package dev.shed.ui.ai

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.setValue
import dev.shed.api.AiEntry

// How the conversation scrolls: it opens at the latest entry and keeps to it
// while new output comes, until you scroll up; a jump goes back down. Pure
// (Compose state only), so tested on the JVM.

/**
 * Whether the transcript keeps to its latest entry. It does from the start;
 * scrolling away by hand stops it, and coming back to the end (by hand or
 * with the jump) starts it again.
 */
class Follow(following: Boolean = true) {
    /** New output scrolls the transcript to its end. */
    var following by mutableStateOf(following)
        private set

    /** A finger is on the list (or its fling still goes): not to be scrolled under it. */
    var touched by mutableStateOf(false)
        private set

    /** The user started dragging the list. */
    fun dragged() {
        touched = true
    }

    /** The list stopped moving, at its end or not. */
    fun settled(atEnd: Boolean) {
        if (touched) {
            touched = false
            following = atEnd
        } else if (atEnd) {
            following = true
        }
    }

    /** Jumped to the latest entry. */
    fun toLatest() {
        touched = false
        following = true
    }

    /** Jumped somewhere else (to one of your messages). */
    fun away() {
        touched = false
        following = false
    }

    /** Whether to scroll to the end now: following, untouched, and something below. */
    fun shouldScroll(moreBelow: Boolean): Boolean = following && !touched && moreBelow

    /** Whether to offer the jump to the latest entry. */
    fun showJump(moreBelow: Boolean): Boolean = !following && moreBelow

    companion object {
        val Saver: Saver<Follow, Boolean> = Saver(save = { it.following }, restore = { Follow(it) })
    }
}

/**
 * Whether a list is at its end: nothing to scroll to, or the last item's end
 * within [slack] pixels of the viewport's (a hair above the end counts).
 */
fun atEnd(canScrollForward: Boolean, lastVisible: Int?, total: Int, lastItemEnd: Int, viewportEnd: Int, slack: Int): Boolean =
    !canScrollForward || (lastVisible != null && lastVisible == total - 1 && lastItemEnd - viewportEnd <= slack)

/** Where your messages are in the conversation. */
fun promptIndices(entries: List<AiEntry>): List<Int> = entries.indices.filter { entries[it] is AiEntry.User }

/**
 * The message of yours before the entry at the top ([top]); scrolled into a
 * message ([inside]), its own start first. `null`: none above.
 */
fun previousPrompt(entries: List<AiEntry>, top: Int, inside: Boolean): Int? =
    promptIndices(entries).lastOrNull { it < top || (it == top && inside) }

/** The message of yours after the entry at the top; `null`: none below (the jump goes to the end). */
fun nextPrompt(entries: List<AiEntry>, top: Int): Int? = promptIndices(entries).firstOrNull { it > top }
