package dev.shed

import dev.shed.api.AiActivity
import dev.shed.api.AiAssistant
import dev.shed.api.AiEntriesChanged
import dev.shed.api.AiEntry
import dev.shed.api.AiEntryAt
import dev.shed.api.AiNotice
import dev.shed.api.AiRun
import dev.shed.api.AiRunState
import dev.shed.api.AiStatus
import dev.shed.api.AiTimer
import dev.shed.api.AiTimerKind
import dev.shed.api.AiTool
import dev.shed.ui.ai.agentLine
import dev.shed.ui.ai.applyEntries
import dev.shed.ui.ai.coarse
import dev.shed.ui.ai.firesLine
import dev.shed.ui.ai.runLine
import dev.shed.ui.ai.runReport
import dev.shed.ui.ai.runTime
import dev.shed.ui.ai.shortSpan
import dev.shed.ui.ai.tickEvery
import dev.shed.ui.ai.timerDetail
import dev.shed.ui.ai.timerLabel
import dev.shed.ui.ai.timerWhen
import dev.shed.ui.ai.tokensShort
import dev.shed.ui.ai.withAttachedFiles
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

/** Sub-agents, background tasks, timers and attached files in words, as the desktop says them. */
class AiRunsTest {
    private val now = 1_763_500_000L

    private fun agent(run: AiRun, name: String = "Agent") = AiTool("t1", name, "Agent: look around", run = run)

    @Test
    fun spans() {
        assertEquals("40 s", coarse(40, false))
        assertEquals("3 min", coarse(200, false))
        assertEquals("4 min", coarse(200, true))
        assertEquals("1 min", coarse(60, true))
        assertEquals("2 h", coarse(7200, false))
        assertEquals("2 h 5 min", coarse(7500, false))
        assertEquals("45 s", shortSpan(45))
        assertEquals("1 min 30 s", shortSpan(90))
        assertEquals("5 min", shortSpan(300))
        assertEquals("1 h 30 min", shortSpan(5400))
        assertEquals("1 h", shortSpan(3600))
    }

    @Test
    fun runLines() {
        val running = AiRun(AiRunState.Running, started = (now - 200).toUInt(), last = "Reading src/main.rs", tools = 4u)
        assertEquals("running 3 min · Reading src/main.rs · 4 tool calls", runLine(agent(running), running, now))
        val one = running.copy(tools = 1u, last = "Starting")
        assertEquals("running 3 min · 1 tool call", runLine(agent(one), one, now))
        // A command in the background: no tool calls of its own counted.
        assertEquals("running 3 min · Reading src/main.rs", runLine(agent(running, "Bash"), running, now))
        val done = AiRun(AiRunState.Done, started = (now - 100).toUInt(), ended = (now - 60).toUInt(), last = "Done", tools = 2u)
        assertEquals("done in 40 s · 2 tool calls", runLine(agent(done), done, now))
        assertEquals("done", runTime(AiRun(AiRunState.Done), now))
        assertEquals("failed", runTime(AiRun(AiRunState.Failed), now))
        assertEquals("stopped", runTime(AiRun(AiRunState.Stopped), now))
        // A state this version does not know (null): as running.
        assertEquals("running", runTime(AiRun(), now))
    }

    @Test
    fun agentDetails() {
        assertEquals("Explore · 12k tokens", agentLine(AiRun(agent = "Explore", tokens = 12_345.0)))
        assertEquals("1.5M tokens", agentLine(AiRun(agent = "", tokens = 1_500_000.0)))
        assertEquals("2M", tokensShort(2_000_000.0))
        val tool = agent(AiRun(AiRunState.Done)).copy(result = "from the result")
        assertEquals("from the result", runReport(tool, tool.run!!))
        assertEquals("its report", runReport(tool, AiRun(AiRunState.Done, report = "its report")))
        assertEquals("", runReport(tool, AiRun(AiRunState.Running)))
    }

    @Test
    fun timers() {
        val wake = AiTimer(AiTimerKind.Wakeup, "check the build", at = (now + 18 * 60).toUInt(), prompt = "Is it green?", tool = "t9")
        assertEquals("Wakes up in 18 min", timerLabel(wake, now))
        val job = AiTimer(AiTimerKind.Job, "tidy", at = (now + 50).toUInt(), recurring = true, every = "Every minute")
        assertEquals("Scheduled job, every minute, next in 50 s", timerLabel(job, now))
        assertEquals("Scheduled job, every minute, next in 1 min", timerLabel(job.copy(at = (now + 60).toUInt()), now))
        assertEquals("Scheduled job, every minute, next in 2 min", timerLabel(job.copy(at = (now + 61).toUInt()), now))
        assertEquals("every minute", timerWhen(job.copy(at = 0u), now))
        val watch = AiTimer(AiTimerKind.Watch, "the log", at = (now + 300).toUInt())
        assertEquals("Watching, ends in 5 min", timerLabel(watch, now))
        assertEquals("Watching, watching", timerLabel(watch.copy(at = 0u), now))
        assertEquals("due now", timerWhen(wake.copy(at = (now - 5).toUInt()), now))
        assertEquals("later", timerWhen(AiTimer(null, "x"), now))
        assertEquals("Wakes up later", timerLabel(AiTimer(null, "x"), now))
        assertEquals("check the build\n\nIt tells Claude: Is it green?", timerDetail(wake))
        assertEquals("tidy", timerDetail(job))
        val status = AiStatus(state = AiActivity.Idle, timers = listOf(wake))
        assertEquals("fires in 18 min", firesLine(AiTool("t9", "ScheduleWakeup", "Wake up later"), status, now))
        assertNull(firesLine(AiTool("t1", "Bash", "ls"), status, now))
    }

    @Test
    fun ticking() {
        val idle = AiStatus(state = AiActivity.Idle)
        assertNull(tickEvery(emptyList(), idle, now))
        fun tool(run: AiRun) = AiEntry.Tool(agent(run))
        // A run in its first minute: every second; later every 30 s; ended: nothing.
        assertEquals(1L, tickEvery(listOf(tool(AiRun(AiRunState.Running, started = (now - 10).toUInt()))), idle, now))
        assertEquals(30L, tickEvery(listOf(tool(AiRun(AiRunState.Running, started = (now - 600).toUInt()))), idle, now))
        assertNull(tickEvery(listOf(tool(AiRun(AiRunState.Done, started = (now - 10).toUInt()))), idle, now))
        // A timer in its last minute: every second; further off: every 30 s; long overdue: nothing.
        fun timers(at: Long) = idle.copy(timers = listOf(AiTimer(AiTimerKind.Wakeup, "x", at = at.toUInt())))
        assertEquals(1L, tickEvery(emptyList(), timers(now + 30), now))
        assertEquals(30L, tickEvery(emptyList(), timers(now + 600), now))
        assertNull(tickEvery(emptyList(), timers(now - 120), now))
        assertNull(tickEvery(emptyList(), timers(0), now))
    }

    @Test
    fun entriesChangeInPlace() {
        val entries = listOf(
            AiEntry.Assistant(AiAssistant("a")),
            AiEntry.Tool(AiTool("t1", "Agent", "Agent: x", run = AiRun(AiRunState.Running))),
            AiEntry.Notice(AiNotice("done")),
        )
        val status = AiStatus(state = AiActivity.Working)
        val newer = AiEntry.Tool(AiTool("t1", "Agent", "Agent: x", run = AiRun(AiRunState.Done)))
        val changed = applyEntries(entries, AiEntriesChanged(listOf(AiEntryAt(1u, newer), AiEntryAt(9u, newer)), status))
        assertEquals(listOf(entries[0], newer, entries[2]), changed)
        // Only indices past the end: the same list.
        assertSame(entries, applyEntries(entries, AiEntriesChanged(listOf(AiEntryAt(3u, newer)), status)))
    }

    @Test
    fun attachedFiles() {
        assertEquals("look\n\nAttached files:\n- /s/a.txt\n- /s/b.pdf", withAttachedFiles("look", listOf("/s/a.txt", "/s/b.pdf")))
        assertEquals("Attached files:\n- /s/a.txt", withAttachedFiles("", listOf("/s/a.txt")))
        assertEquals("look", withAttachedFiles("look", emptyList()))
    }
}
