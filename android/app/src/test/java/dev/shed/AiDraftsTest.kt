package dev.shed

import dev.shed.ui.ai.mergedDraft
import org.junit.Assert.assertEquals
import org.junit.Test

class AiDraftsTest {
    @Test
    fun aDraftMergesAtTheEndAfterABlankLine() {
        assertEquals("and the tests", mergedDraft("", "and the tests"))
        assertEquals("and the tests", mergedDraft("  \n", "and the tests"))
        assertEquals("Fix the build\n\nand the tests", mergedDraft("Fix the build", "and the tests"))
        assertEquals("Fix the build\n\nand the tests", mergedDraft("Fix the build\n\n", "and the tests"))
    }
}
