package dev.shed.ui.common

import android.content.Context
import dev.shed.R
import dev.shed.api.Bench
import dev.shed.api.ErrorCode
import dev.shed.api.Tab
import dev.shed.api.WorkspaceState
import dev.shed.net.ApiException
import java.io.IOException
import java.text.DateFormat
import java.util.Date

/** Now, in Unix seconds. */
fun nowSeconds(): Long = System.currentTimeMillis() / 1000

/**
 * A range of [text] the API gives (highlights, search ranges): positions in
 * UTF-16 code units, which is how Kotlin strings index, so used as they are;
 * only clamped to the text. `null` when nothing is left of it.
 */
fun textRange(text: String, start: Int, end: Int): IntRange? {
    val s = start.coerceIn(0, text.length)
    val e = end.coerceIn(s, text.length)
    return if (e > s) s until e else null
}

/**
 * The character at [pos] (a UTF-16 position, as quick open's hits give),
 * whole: both halves of a surrogate pair (most emoji). `null` past the end.
 */
fun charRange(text: String, pos: Int): IntRange? {
    if (pos !in text.indices) return null
    val start = if (pos > 0 && text[pos].isLowSurrogate() && text[pos - 1].isHighSurrogate()) pos - 1 else pos
    return start until start + Character.charCount(text.codePointAt(start))
}

/** Every start, end pair of [ranges] (`SearchHit.ranges`) as a range of [text]. */
fun textRanges(text: String, ranges: List<UInt>): List<IntRange> =
    ranges.chunked(2).mapNotNull { if (it.size == 2) textRange(text, it[0].toInt(), it[1].toInt()) else null }

/** How many terminal cells a character takes: 2 for wide (CJK, emoji), else 1. */
fun cellWidth(codePoint: Int): Int = when {
    codePoint < 0x1100 -> 1
    codePoint in 0x1100..0x115F || codePoint in 0x2E80..0xA4CF || codePoint in 0xAC00..0xD7A3 ||
        codePoint in 0xF900..0xFAFF || codePoint in 0xFE30..0xFE4F || codePoint in 0xFF00..0xFF60 ||
        codePoint in 0xFFE0..0xFFE6 || codePoint in 0x1F300..0x1F64F || codePoint in 0x1F900..0x1F9FF ||
        codePoint in 0x20000..0x3FFFD -> 2
    else -> 1
}

/** "now", "5 min ago", "3 h ago", "2 d ago", or the date. */
fun ago(context: Context, seconds: Long): String {
    if (seconds <= 0) return context.getString(R.string.time_unknown)
    val diff = nowSeconds() - seconds
    return when {
        diff < 60 -> context.getString(R.string.time_now)
        diff < 3600 -> context.getString(R.string.time_minutes_ago, diff / 60)
        diff < 86400 -> context.getString(R.string.time_hours_ago, diff / 3600)
        diff < 7 * 86400 -> context.getString(R.string.time_days_ago, diff / 86400)
        else -> DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(seconds * 1000))
    }
}

/** A time of day (today) or date and time. */
fun clock(seconds: Long): String {
    if (seconds <= 0) return ""
    val date = Date(seconds * 1000)
    val today = nowSeconds() / 86400 == seconds / 86400
    return if (today) {
        DateFormat.getTimeInstance(DateFormat.SHORT).format(date)
    } else {
        DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(date)
    }
}

/** "4s", "2m 5s", "1h 12m". */
fun span(seconds: Long): String = when {
    seconds < 60 -> "${seconds}s"
    seconds < 3600 -> "${seconds / 60}m ${seconds % 60}s"
    seconds < 86400 -> "${seconds / 3600}h ${(seconds % 3600) / 60}m"
    else -> "${seconds / 86400}d ${(seconds % 86400) / 3600}h"
}

/** A path with the home folder as `~`. */
fun tildePath(path: String, home: String): String =
    if (home.isNotEmpty() && (path == home || path.startsWith("$home/"))) "~" + path.removePrefix(home) else path

/** What went wrong, for people. */
fun errorText(context: Context, e: Throwable): String = when (e) {
    is ApiException -> {
        val base = context.getString(
            when (e.code) {
                ErrorCode.Unauthorized -> R.string.error_unauthorized
                ErrorCode.Forbidden -> R.string.error_forbidden
                ErrorCode.NotFound -> R.string.error_not_found
                ErrorCode.Offline -> R.string.error_offline
                ErrorCode.Invalid -> R.string.error_invalid
                ErrorCode.Unsupported -> R.string.error_unsupported
                ErrorCode.RateLimited -> R.string.error_rate_limited
                ErrorCode.WrongCode -> R.string.error_wrong_code
                ErrorCode.Timeout -> R.string.error_timeout
                ErrorCode.Failed -> R.string.error_failed
            },
        )
        val detail = e.error.message.orEmpty()
        if (detail.isBlank()) base else "$base ($detail)"
    }
    is IOException -> context.getString(R.string.error_network, e.message ?: e.javaClass.simpleName)
    else -> e.message ?: e.javaClass.simpleName
}

fun WorkspaceState.bench(id: String): Bench? = benches.firstOrNull { it.id == id }

/** A running tab and its bench. */
fun WorkspaceState.findTab(tab: String): Pair<Bench, Tab>? {
    for (bench in benches) bench.tabs.firstOrNull { it.id == tab }?.let { return bench to it }
    return null
}
