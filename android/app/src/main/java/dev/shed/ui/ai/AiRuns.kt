package dev.shed.ui.ai

import dev.shed.api.AiEntriesChanged
import dev.shed.api.AiEntry
import dev.shed.api.AiRun
import dev.shed.api.AiRunState
import dev.shed.api.AiStatus
import dev.shed.api.AiTimer
import dev.shed.api.AiTimerKind
import dev.shed.api.AiTool

// Sub-agents, background tasks and timers in words, as the desktop's AI tab
// says them (`crates/shed-app/src/ai_view.rs`). Pure, so tested on the JVM.

/** A span of time, short: `45 s`, `1 min 30 s`, `5 min`, `1 h 30 min` (`shed_ai::span`). */
fun shortSpan(secs: Long): String {
    val s = secs.coerceAtLeast(0)
    return when {
        s < 60 -> "$s s"
        s < 300 && s % 60 != 0L -> "${s / 60} min ${s % 60} s"
        s < 3600 -> "${(s + 30) / 60} min"
        (s / 60) % 60 == 0L -> "${s / 3600} h"
        else -> "${s / 3600} h ${(s / 60) % 60} min"
    }
}

/**
 * A span still going, coarse past a minute (counted every second only in the
 * first minute): `40 s`, `3 min`, `2 h 5 min`; whole minutes rounded down
 * (time gone) or up (time left, [up]).
 */
fun coarse(secs: Long, up: Boolean): String {
    val s = secs.coerceAtLeast(0)
    val minutes = if (up) (s + 59) / 60 else s / 60
    return when {
        s < 60 -> "$s s"
        minutes < 60 -> "$minutes min"
        minutes % 60 == 0L -> "${minutes / 60} h"
        else -> "${minutes / 60} h ${minutes % 60} min"
    }
}

/** A wake-up past its time by more than this is not counted down any more. */
const val OVERDUE = 60L

/** A run's state, missing when this version does not know it: as running. */
val AiRun.stateOrRunning: AiRunState get() = state ?: AiRunState.Running

val AiRun.running: Boolean get() = stateOrRunning == AiRunState.Running

/** A sub-agent (rather than a command or watch in the background). */
val AiTool.isAgent: Boolean get() = name == "Agent" || name == "Task"

/** A run's state in a word, and for how long: `running 3 min`, `done in 40 s`. */
fun runTime(run: AiRun, now: Long): String {
    val started = run.started?.toLong() ?: 0
    val ended = run.ended?.toLong() ?: 0
    return when (run.stateOrRunning) {
        AiRunState.Running -> if (started > 0) "running " + coarse(now - started, false) else "running"
        AiRunState.Done -> if (started > 0 && ended >= started) "done in " + shortSpan(ended - started) else "done"
        AiRunState.Failed -> "failed"
        AiRunState.Stopped -> "stopped"
    }
}

/**
 * A sub-agent's or background task's line after its tool's: how it is doing,
 * what it does now, how many tool calls (`running 3 min · Reading x · 4 tool calls`).
 */
fun runLine(tool: AiTool, run: AiRun, now: Long): String {
    val parts = mutableListOf(runTime(run, now))
    val last = run.last.orEmpty()
    if (run.running && last.isNotEmpty() && last != "Starting") parts += last
    val tools = run.tools?.toLong() ?: 0
    if (tool.isAgent && tools > 0) parts += "$tools tool call" + if (tools == 1L) "" else "s"
    return parts.joinToString(" · ")
}

/** A run's own words, without the tool's name: an agent's task (`Agent: find x` → `find x`), a command as it is. */
fun runTitle(tool: AiTool): String {
    val summary = tool.summary
    for (prefix in listOf("Agent:", "Task:")) {
        if (summary.startsWith(prefix)) return summary.removePrefix(prefix).trim().ifEmpty { summary }
    }
    return summary
}

/** A run's state in a word: `running`, `done`, `failed`, `stopped`. */
fun runState(run: AiRun): String = when (run.stateOrRunning) {
    AiRunState.Running -> "running"
    AiRunState.Done -> "done"
    AiRunState.Failed -> "failed"
    AiRunState.Stopped -> "stopped"
}

/**
 * What follows a run's state on its status line: for how long, how many tool
 * calls, the kind of agent (`1 min · 6 tool calls · Explore`).
 */
fun runStatus(tool: AiTool, run: AiRun, now: Long): String {
    val parts = mutableListOf<String>()
    val started = run.started?.toLong() ?: 0
    val ended = run.ended?.toLong() ?: 0
    if (started > 0) {
        if (run.running) {
            parts += coarse(now - started, false)
        } else if (ended >= started) {
            parts += shortSpan(ended - started)
        }
    }
    val tools = run.tools?.toLong() ?: 0
    if (tool.isAgent && tools > 0) parts += "$tools tool call" + if (tools == 1L) "" else "s"
    run.agent?.takeIf { it.isNotEmpty() && it != "general-purpose" }?.let { parts += it }
    return parts.joinToString(" · ")
}

/** What a run does now (`Read LogbookScreen.kt`); null once it ended, or while it starts. */
fun runStep(run: AiRun): String? = run.last?.takeIf { run.running && it.isNotBlank() && it != "Starting" }

/** Tools whose run goes on: sub-agents first, then commands in the background, each in the conversation's order. */
fun runningTools(entries: List<AiEntry>): List<AiTool> {
    val tools = entries.mapNotNull { (it as? AiEntry.Tool)?.content?.takeIf { t -> t.run?.running == true } }
    return tools.filter { it.isAgent } + tools.filterNot { it.isAgent }
}

/**
 * The strip's line for several things going on in the background:
 * `2 agents running · 1 background command · 2 timers`.
 */
fun backgroundSummary(agents: Int, commands: Int, timers: Int): String = listOfNotNull(
    agents.takeIf { it > 0 }?.let { "$it agent" + (if (it == 1) "" else "s") + " running" },
    commands.takeIf { it > 0 }?.let { "$it background command" + if (it == 1) "" else "s" },
    timers.takeIf { it > 0 }?.let { "$it timer" + if (it == 1) "" else "s" },
).joinToString(" · ")

/** Tokens, short: `12k`, `1.5M`, `2M`. */
fun tokensShort(n: Double): String {
    val t = n.toLong()
    return when {
        t >= 1_000_000 && t % 1_000_000 == 0L -> "${t / 1_000_000}M"
        t >= 1_000_000 -> "%.1fM".format(java.util.Locale.ROOT, t / 1_000_000.0)
        else -> "${t / 1000}k"
    }
}

/** An opened sub-agent's kind and tokens: `Explore · 12k tokens`. */
fun agentLine(run: AiRun): String = listOfNotNull(
    run.agent?.takeIf { it.isNotEmpty() },
    run.tokens?.takeIf { it > 0 }?.let { tokensShort(it) + " tokens" },
).joinToString(" · ")

/** What an ended sub-agent reported: its report, or else its tool's result. */
fun runReport(tool: AiTool, run: AiRun): String =
    run.report?.takeIf { it.isNotEmpty() } ?: tool.result?.takeIf { !run.running }.orEmpty()

/** When a timer fires, as a person says it: `in 18 min`, `every minute, next in 1 min`. */
fun timerWhen(timer: AiTimer, now: Long): String {
    val at = timer.at?.toLong() ?: 0
    val every = timer.every.orEmpty()
    fun left(at: Long) = if (at > now) "in " + coarse(at - now, true) else "due now"
    return when {
        timer.kind == AiTimerKind.Watch && at > 0 -> "ends " + left(at)
        timer.kind == AiTimerKind.Watch -> "watching"
        every.isNotEmpty() && at > now -> every.lowercase() + ", next " + left(at)
        every.isNotEmpty() -> every.lowercase()
        at > 0 -> left(at)
        else -> "later"
    }
}

/** A timer's chip: `Wakes up in 18 min`, `Scheduled job, every minute, next in 1 min`, `Watching, ends in 5 min`. */
fun timerLabel(timer: AiTimer, now: Long): String = when (timer.kind) {
    AiTimerKind.Job -> "Scheduled job, " + timerWhen(timer, now)
    AiTimerKind.Watch -> "Watching, " + timerWhen(timer, now)
    // A kind this version does not know reads as null: a wake-up's words fit any.
    AiTimerKind.Wakeup, null -> "Wakes up " + timerWhen(timer, now)
}

/** What a timer is for and what it tells Claude, to read in full. */
fun timerDetail(timer: AiTimer): String {
    val prompt = timer.prompt.orEmpty()
    return if (prompt.isEmpty()) timer.what else timer.what + "\n\nIt tells Claude: " + prompt
}

/** After a tool's line, when a timer it set fires (`fires in 18 min`); null when it set none. */
fun firesLine(tool: AiTool, status: AiStatus?, now: Long): String? =
    status?.timers.orEmpty().firstOrNull { it.tool == tool.id }?.let { "fires " + timerWhen(it, now) }

/** Runs going on in a conversation (tools whose run is running). */
fun runningRuns(entries: List<AiEntry>): List<AiRun> =
    entries.mapNotNull { (it as? AiEntry.Tool)?.content?.run?.takeIf { r -> r.running } }

/**
 * How often what is shown counts on, in seconds: every second for a run in its
 * first minute or a timer in its last, every 30 s for the rest (runs going on
 * for long, timers further off), null when nothing counts.
 */
fun tickEvery(entries: List<AiEntry>, status: AiStatus?, now: Long): Long? {
    val runs = runningRuns(entries)
    val counted = status?.timers.orEmpty().filter { (it.at?.toLong() ?: 0) > 0 && it.at!!.toLong() + OVERDUE > now }
    val newest = runs.maxOfOrNull { it.started?.toLong() ?: 0 } ?: 0
    val fresh = runs.isNotEmpty() && newest + 60 > now
    return when {
        fresh || counted.any { it.at!!.toLong() <= now + 60 } -> 1
        runs.isNotEmpty() || counted.isNotEmpty() -> 30
        else -> null
    }
}

/** A conversation with entries changed in place (`Event.AiEntries`); indices past its end are left out. */
fun applyEntries(entries: List<AiEntry>, changed: AiEntriesChanged): List<AiEntry> {
    if (changed.entries.none { it.index.toInt() in entries.indices }) return entries
    val out = entries.toMutableList()
    for (at in changed.entries) {
        val i = at.index.toLong()
        if (i in 0 until out.size) out[i.toInt()] = at.entry
    }
    return out
}

/** The prompt with the paths of files it carries, as the desktop sends them. */
fun withAttachedFiles(text: String, paths: List<String>): String {
    if (paths.isEmpty()) return text
    return text + (if (text.isEmpty()) "" else "\n\n") + "Attached files:\n" + paths.joinToString("\n") { "- $it" }
}
