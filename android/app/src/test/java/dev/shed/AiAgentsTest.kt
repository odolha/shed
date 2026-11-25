package dev.shed

import dev.shed.api.AiAssistant
import dev.shed.api.AiEntry
import dev.shed.api.AiRun
import dev.shed.api.AiRunState
import dev.shed.api.AiTool
import dev.shed.ui.ai.UserMenu
import dev.shed.ui.ai.backgroundSummary
import dev.shed.ui.ai.runState
import dev.shed.ui.ai.runStatus
import dev.shed.ui.ai.runStep
import dev.shed.ui.ai.runTitle
import dev.shed.ui.ai.runningTools
import dev.shed.ui.ai.userMenu
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** How running agents and background commands read on the phone, and what a long press on your message offers. */
class AiAgentsTest {
    private val now = 1_763_500_000L

    private fun tool(id: String, name: String, summary: String, run: AiRun?) = AiTool(id, name, summary, run = run)

    @Test
    fun titleDropsTheToolsName() {
        assertEquals("find the query", runTitle(tool("a", "Agent", "Agent: find the query", null)))
        assertEquals("review it", runTitle(tool("a", "Task", "Task:  review it", null)))
        assertEquals("$ ./gradlew lint", runTitle(tool("b", "Bash", "$ ./gradlew lint", null)))
        assertEquals("Agent:", runTitle(tool("a", "Agent", "Agent:", null)))
    }

    @Test
    fun statusSaysHowLongHowManyAndWhatKind() {
        val running = AiRun(AiRunState.Running, agent = "Explore", started = (now - 95).toUInt(), tools = 6u)
        val agent = tool("a", "Agent", "Agent: x", running)
        assertEquals("running", runState(running))
        assertEquals("1 min · 6 tool calls · Explore", runStatus(agent, running, now))
        val done = AiRun(AiRunState.Done, agent = "general-purpose", started = (now - 100).toUInt(), ended = (now - 60).toUInt(), tools = 1u)
        assertEquals("done", runState(done))
        assertEquals("40 s · 1 tool call", runStatus(agent, done, now))
        assertEquals("failed", runState(AiRun(AiRunState.Failed)))
        assertEquals("stopped", runState(AiRun(AiRunState.Stopped)))
        // A command counts no tool calls; nothing known, nothing said.
        val command = AiRun(AiRunState.Running, background = true, tools = 3u)
        assertEquals("", runStatus(tool("b", "Bash", "$ make", command), command, now))
        // A state this version does not know reads as running.
        assertEquals("running", runState(AiRun(null)))
    }

    @Test
    fun stepIsWhatItDoesWhileRunning() {
        assertEquals("Read a.kt", runStep(AiRun(AiRunState.Running, last = "Read a.kt")))
        assertNull(runStep(AiRun(AiRunState.Running, last = "Starting")))
        assertNull(runStep(AiRun(AiRunState.Running)))
        assertNull(runStep(AiRun(AiRunState.Done, last = "Read a.kt")))
    }

    @Test
    fun runningAgentsComeFirst() {
        val entries = listOf(
            AiEntry.Tool(tool("c1", "Bash", "$ make", AiRun(AiRunState.Running, background = true))),
            AiEntry.Assistant(AiAssistant("meanwhile")),
            AiEntry.Tool(tool("a1", "Agent", "Agent: one", AiRun(AiRunState.Running))),
            AiEntry.Tool(tool("a2", "Agent", "Agent: done", AiRun(AiRunState.Done))),
            AiEntry.Tool(tool("t", "Read", "Read x", null)),
            AiEntry.Tool(tool("a3", "Task", "Task: two", AiRun(null))),
        )
        assertEquals(listOf("a1", "a3", "c1"), runningTools(entries).map { it.id })
    }

    @Test
    fun summaryCountsEachKind() {
        assertEquals("2 agents running · 1 background command", backgroundSummary(2, 1, 0))
        assertEquals("1 agent running · 2 background commands · 1 timer", backgroundSummary(1, 2, 1))
        assertEquals("3 timers", backgroundSummary(0, 0, 3))
        assertEquals("", backgroundSummary(0, 0, 0))
    }

    @Test
    fun yourMessagesMenu() {
        assertEquals(listOf(UserMenu.Copy, UserMenu.Select, UserMenu.Edit, UserMenu.SendAgain, UserMenu.Revert), userMenu(canChange = true))
        // View only: nothing that changes the conversation.
        assertEquals(listOf(UserMenu.Copy, UserMenu.Select), userMenu(canChange = false))
        assertEquals(listOf(UserMenu.Revert), UserMenu.entries.filter { it.danger })
    }
}
