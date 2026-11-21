package dev.shed

import dev.shed.api.AiSession
import dev.shed.ui.ai.laterSessions
import org.junit.Assert.assertEquals
import org.junit.Test

class AiSessionsTest {
    private fun session(id: String, parent: String?) = AiSession(id = id, title = id, modified = 0u, prompts = 1u, parent = parent)

    @Test
    fun deletingASessionTakesThoseGoingOnFromItAlong() {
        val all = listOf(session("a", null), session("b", "a"), session("bb", "b"), session("c", "a"), session("z", null))
        assertEquals(listOf("b", "bb", "c"), laterSessions(all, "a").map { it.id }.sorted())
        assertEquals(listOf("bb"), laterSessions(all, "b").map { it.id })
        assertEquals(emptyList<String>(), laterSessions(all, "z").map { it.id })
    }
}
