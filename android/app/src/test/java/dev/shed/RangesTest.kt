package dev.shed

import dev.shed.ui.common.charRange
import dev.shed.ui.common.textRange
import dev.shed.ui.common.textRanges
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Positions from the API count UTF-16 code units, as Kotlin strings do. */
class RangesTest {
    // "😀" is two UTF-16 code units (a surrogate pair).
    private val line = "a😀 = \"b\""

    @Test
    fun rangesAfterAnEmojiAreUsedAsGiven() {
        // `"b"` starts at UTF-16 position 6 (a, two for the emoji, space, =, space).
        val r = textRange(line, 6, 9)!!
        assertEquals("\"b\"", line.substring(r))
        assertEquals("😀", line.substring(textRange(line, 1, 3)!!))
    }

    @Test
    fun searchRangesComeInPairs() {
        val text = "🦀 fn main() { main }"
        val found = textRanges(text, listOf(6u, 10u, 15u, 19u, 99u))
        assertEquals(listOf("main", "main"), found.map { text.substring(it) })
    }

    @Test
    fun rangesAreClampedToTheText() {
        assertEquals("b\"", line.substring(textRange(line, 7, 50)!!))
        assertNull(textRange(line, 9, 12))
        assertNull(textRange(line, 4, 4))
    }

    @Test
    fun aQuickOpenHitTakesTheWholeCharacter() {
        val path = "docs/😀/b.md"
        assertEquals("😀", path.substring(charRange(path, 5)!!))
        // A hit on the second half of the pair takes the pair too.
        assertEquals("😀", path.substring(charRange(path, 6)!!))
        assertEquals("b", path.substring(charRange(path, 8)!!))
        assertNull(charRange(path, path.length))
    }
}
