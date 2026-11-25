package dev.shed

import dev.shed.ui.common.ScrollbarMath
import dev.shed.ui.common.ScrollbarMath.Layout
import dev.shed.ui.common.ScrollbarMath.Seen
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The scrollbar's estimate of where a lazy list is, and where a drag takes it. */
class ScrollbarTest {
    /** 100 items of 100 px in a 1000 px viewport, the first one in view at [first], [into] pixels scrolled into it. */
    private fun even(first: Int, into: Int = 0, total: Int = 100): Layout {
        val seen = (0 until 11).map { Seen(first + it, it * 100 - into, 100) }.filter { it.index < total }
        return Layout(seen, total, viewport = 1000, atStart = first == 0 && into == 0, atEnd = first + 10 >= total)
    }

    @Test
    fun evenItemsGiveAnExactPlace() {
        val l = even(45)
        assertEquals(100f, ScrollbarMath.stride(l), 0.01f)
        assertEquals(10_000f, ScrollbarMath.content(l), 0.01f)
        assertEquals(0.5f, ScrollbarMath.fraction(l), 0.001f)
    }

    @Test
    fun theEndsAreTheEnds() {
        assertEquals(0f, ScrollbarMath.fraction(even(0)), 0f)
        assertEquals(1f, ScrollbarMath.fraction(even(90)), 0f)
    }

    @Test
    fun thumbIsTheViewportsShareOfTheTrack() {
        val thumb = assertNotNull(ScrollbarMath.thumb(even(45), track = 1000f, minLength = 20f))
        assertEquals(100f, thumb.length, 0.01f)
        assertEquals(450f, thumb.top, 0.5f)
    }

    private fun <T> assertNotNull(value: T?): T {
        org.junit.Assert.assertNotNull(value)
        return value!!
    }

    @Test
    fun aLongListKeepsAThumbYouCanHold() {
        val thumb = assertNotNull(ScrollbarMath.thumb(even(5000, total = 10_000), track = 1000f, minLength = 48f))
        assertEquals(48f, thumb.length, 0.01f)
        assertTrue(thumb.top + thumb.length <= 1000f)
    }

    @Test
    fun nothingToScrollNoThumb() {
        val l = Layout(listOf(Seen(0, 0, 100), Seen(1, 100, 100)), 2, viewport = 1000, atStart = true, atEnd = true)
        assertNull(ScrollbarMath.thumb(l, 1000f, 20f))
        assertNull(ScrollbarMath.thumb(Layout(emptyList(), 0, 1000), 1000f, 20f))
    }

    @Test
    fun spacingAndPaddingCount() {
        // 50 items of 90 px, 10 px apart, 40 px of padding: 50 * 100 - 10 + 40.
        val l = Layout(listOf(Seen(10, -30, 90), Seen(11, 70, 90)), 50, viewport = 1000, spacing = 10, padding = 40)
        assertEquals(100f, ScrollbarMath.stride(l), 0.01f)
        assertEquals(5030f, ScrollbarMath.content(l), 0.01f)
        assertEquals(1030f / 4030f, ScrollbarMath.fraction(l), 0.001f)
    }

    @Test
    fun itemsOfDifferentHeightsAreAveraged() {
        // A tall item and two short ones in view: their mean stands in for the rest.
        val l = Layout(listOf(Seen(3, -50, 600), Seen(4, 550, 100), Seen(5, 650, 200)), 20, viewport = 800)
        assertEquals(300f, ScrollbarMath.stride(l), 0.01f)
        val f = ScrollbarMath.fraction(l)
        assertTrue(f > 0f && f < 1f)
        assertEquals((3 * 300f + 50f) / (6000f - 800f), f, 0.001f)
    }

    @Test
    fun aDragGoesToTheItemAtThatShare() {
        val l = even(0)
        // Halfway: 4500 px of 9000 down, item 45 at its start.
        assertEquals(45 to 0, ScrollbarMath.target(l, 0.5f))
        assertEquals(0 to 0, ScrollbarMath.target(l, 0f))
        assertEquals(22 to 50, ScrollbarMath.target(l, 0.25f))
        // At (and past) the end: the last item, which the list stops at the end.
        assertEquals(90, ScrollbarMath.target(l, 1f).first)
        assertEquals(90, ScrollbarMath.target(l, 3f).first)
        assertEquals(0 to 0, ScrollbarMath.target(Layout(emptyList(), 0, 1000), 0.5f))
    }

    @Test
    fun theThumbsTopIsAShareOfItsWay() {
        assertEquals(0.5f, ScrollbarMath.dragFraction(450f, 100f, 1000f), 0.001f)
        assertEquals(0f, ScrollbarMath.dragFraction(-20f, 100f, 1000f), 0f)
        assertEquals(1f, ScrollbarMath.dragFraction(990f, 100f, 1000f), 0f)
        assertEquals(0f, ScrollbarMath.dragFraction(0f, 1000f, 1000f), 0f)
    }
}
