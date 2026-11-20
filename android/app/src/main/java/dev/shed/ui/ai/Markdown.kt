package dev.shed.ui.ai

import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle

/** Markdown as blocks to lay out natively. */
sealed interface MdBlock {
    data class Paragraph(val text: AnnotatedString) : MdBlock
    data class Heading(val level: Int, val text: AnnotatedString) : MdBlock
    data class Code(val lang: String, val code: String) : MdBlock
    data class Quote(val blocks: List<MdBlock>) : MdBlock
    data class ListBlock(val ordered: Boolean, val start: Int, val items: List<List<MdBlock>>) : MdBlock
    data class Table(val header: List<AnnotatedString>, val rows: List<List<AnnotatedString>>) : MdBlock
    data object Rule : MdBlock
}

/** How inline code and links look. */
data class MdStyle(
    val code: SpanStyle = SpanStyle(fontFamily = FontFamily.Monospace),
    val link: SpanStyle = SpanStyle(textDecoration = TextDecoration.Underline),
)

/**
 * A small Markdown reader for Claude's answers: headings, paragraphs,
 * emphasis, inline code, links, lists (nested), quotes, fenced code, tables
 * and rules. Not all of CommonMark; what Claude writes.
 */
object Markdown {
    private val fence = Regex("^ {0,3}(`{3,}|~{3,})\\s*([^`\\s]*).*$")
    private val heading = Regex("^ {0,3}(#{1,6})\\s+(.*?)\\s*#*\\s*$")
    private val rule = Regex("^ {0,3}([-*_])(\\s*\\1){2,}\\s*$")
    private val quote = Regex("^ {0,3}>\\s?(.*)$")
    private val listItem = Regex("^(\\s*)([-*+]|\\d{1,9}[.)])(\\s+)(.*)$")
    private val tableSep = Regex("^\\s*\\|?\\s*:?-{1,}:?\\s*(\\|\\s*:?-{1,}:?\\s*)*\\|?\\s*$")
    private val url = Regex("https?://[^\\s<>()\\[\\]]+[^\\s<>()\\[\\].,;:!?'\"*_]")

    fun parse(text: String, style: MdStyle = MdStyle()): List<MdBlock> =
        blocks(text.replace("\r\n", "\n").replace('\t', ' ').split('\n'), style)

    private fun closesFence(line: String, marker: String): Boolean {
        val t = line.trim()
        return t.length >= marker.length && t.all { it == marker[0] }
    }

    private fun indentOf(line: String) = line.length - line.trimStart(' ').length

    private fun startsBlock(line: String): Boolean =
        fence.matches(line) || heading.matches(line) || quote.matches(line) || listItem.matches(line) || rule.matches(line)

    private fun blocks(lines: List<String>, style: MdStyle): List<MdBlock> {
        val out = ArrayList<MdBlock>()
        var i = 0
        while (i < lines.size) {
            val line = lines[i]
            if (line.isBlank()) {
                i++
                continue
            }
            val f = fence.matchEntire(line)
            if (f != null) {
                val marker = f.groupValues[1]
                val code = ArrayList<String>()
                i++
                while (i < lines.size && !closesFence(lines[i], marker)) {
                    code.add(lines[i])
                    i++
                }
                i++ // the closing fence
                out.add(MdBlock.Code(f.groupValues[2], code.joinToString("\n")))
                continue
            }
            val h = heading.matchEntire(line)
            if (h != null) {
                out.add(MdBlock.Heading(h.groupValues[1].length, inline(h.groupValues[2], style)))
                i++
                continue
            }
            if (rule.matches(line)) {
                out.add(MdBlock.Rule)
                i++
                continue
            }
            if (quote.matches(line)) {
                val inner = ArrayList<String>()
                while (i < lines.size && lines[i].isNotBlank()) {
                    val m = quote.matchEntire(lines[i])
                    inner.add(m?.groupValues?.get(1) ?: lines[i])
                    i++
                }
                out.add(MdBlock.Quote(blocks(inner, style)))
                continue
            }
            val item = listItem.matchEntire(line)
            if (item != null) {
                val (block, next) = list(lines, i, style)
                out.add(block)
                i = next
                continue
            }
            if ('|' in line && i + 1 < lines.size && tableSep.matches(lines[i + 1]) && '-' in lines[i + 1]) {
                val header = cells(line).map { inline(it, style) }
                i += 2
                val rows = ArrayList<List<AnnotatedString>>()
                while (i < lines.size && lines[i].isNotBlank() && '|' in lines[i]) {
                    rows.add(cells(lines[i]).map { inline(it, style) })
                    i++
                }
                out.add(MdBlock.Table(header, rows))
                continue
            }
            val para = ArrayList<String>()
            while (i < lines.size && lines[i].isNotBlank() && (para.isEmpty() || !startsBlock(lines[i]))) {
                para.add(lines[i].trim())
                i++
            }
            out.add(MdBlock.Paragraph(inline(joinLines(para), style)))
        }
        return out
    }

    /** Lines of a paragraph: soft breaks become spaces; two trailing spaces or `\` keep the break. */
    private fun joinLines(lines: List<String>): String = buildString {
        lines.forEachIndexed { index, l ->
            if (index > 0) append(if (lines[index - 1].endsWith("\\")) "\n" else " ")
            append(l.removeSuffix("\\"))
        }
    }

    private fun cells(line: String): List<String> {
        var t = line.trim()
        if (t.startsWith("|")) t = t.drop(1)
        if (t.endsWith("|")) t = t.dropLast(1)
        return t.split('|').map { it.trim() }
    }

    private fun list(lines: List<String>, from: Int, style: MdStyle): Pair<MdBlock, Int> {
        val first = listItem.matchEntire(lines[from])!!
        val indent = first.groupValues[1].length
        val ordered = first.groupValues[2].first().isDigit()
        val start = if (ordered) first.groupValues[2].dropLast(1).toIntOrNull() ?: 1 else 1
        val items = ArrayList<List<MdBlock>>()
        var i = from
        while (i < lines.size) {
            val m = listItem.matchEntire(lines[i]) ?: break
            if (m.groupValues[1].length != indent || m.groupValues[2].first().isDigit() != ordered) break
            val offset = indent + m.groupValues[2].length + m.groupValues[3].length.coerceAtMost(4)
            val body = arrayListOf(m.groupValues[4])
            i++
            while (i < lines.size) {
                val l = lines[i]
                if (l.isBlank()) {
                    val next = (i + 1 until lines.size).firstOrNull { lines[it].isNotBlank() }
                    if (next != null && indentOf(lines[next]) >= offset) {
                        body.add("")
                        i++
                        continue
                    }
                    break
                }
                val lead = indentOf(l)
                when {
                    lead >= offset -> body.add(l.drop(offset))
                    lead > indent && listItem.matches(l) -> body.add(l.drop(lead.coerceAtMost(offset)))
                    listItem.matches(l) || startsBlock(l) -> break
                    else -> body.add(l.trim()) // a lazy continuation of the item's paragraph
                }
                i++
            }
            items.add(blocks(body, style))
            // A blank line between items keeps the list going.
            var j = i
            while (j < lines.size && lines[j].isBlank()) j++
            val nextItem = lines.getOrNull(j)?.let { listItem.matchEntire(it) }
            if (j > i && nextItem != null && nextItem.groupValues[1].length == indent) i = j
        }
        return MdBlock.ListBlock(ordered, start, items) to i
    }

    /** Inline Markdown as styled text. */
    fun inline(text: String, style: MdStyle = MdStyle()): AnnotatedString = buildAnnotatedString { emit(this, text, style) }

    private fun emit(b: AnnotatedString.Builder, s: String, style: MdStyle) {
        var i = 0
        val plain = StringBuilder()
        fun flush() {
            if (plain.isNotEmpty()) {
                b.append(plain.toString())
                plain.clear()
            }
        }
        while (i < s.length) {
            val c = s[i]
            when {
                c == '\\' && i + 1 < s.length && s[i + 1] in "\\`*_{}[]()#+-.!|~<>" -> {
                    plain.append(s[i + 1])
                    i += 2
                }
                c == '`' -> {
                    var run = 0
                    while (i + run < s.length && s[i + run] == '`') run++
                    val ticks = "`".repeat(run)
                    val end = s.indexOf(ticks, i + run)
                    if (end < 0) {
                        plain.append(ticks)
                        i += run
                    } else {
                        flush()
                        var code = s.substring(i + run, end)
                        if (code.length > 2 && code.startsWith(' ') && code.endsWith(' ')) code = code.substring(1, code.length - 1)
                        b.withStyle(style.code) { append(code) }
                        i = end + run
                    }
                }
                (c == '*' || c == '_') && s.startsWith("$c$c", i) -> {
                    val end = s.indexOf("$c$c", i + 2)
                    if (end > i + 2) {
                        flush()
                        b.withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { emit(b, s.substring(i + 2, end), style) }
                        i = end + 2
                    } else {
                        plain.append("$c$c")
                        i += 2
                    }
                }
                c == '~' && s.startsWith("~~", i) -> {
                    val end = s.indexOf("~~", i + 2)
                    if (end > i + 2) {
                        flush()
                        b.withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { emit(b, s.substring(i + 2, end), style) }
                        i = end + 2
                    } else {
                        plain.append("~~")
                        i += 2
                    }
                }
                (c == '*' || c == '_') && emphasisOpens(s, i) -> {
                    val end = emphasisCloses(s, i + 1, c)
                    if (end > 0) {
                        flush()
                        b.withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { emit(b, s.substring(i + 1, end), style) }
                        i = end + 1
                    } else {
                        plain.append(c)
                        i++
                    }
                }
                c == '[' -> {
                    val close = matching(s, i, '[', ']')
                    if (close > 0 && close + 1 < s.length && s[close + 1] == '(') {
                        val paren = matching(s, close + 1, '(', ')')
                        if (paren > 0) {
                            flush()
                            val target = s.substring(close + 2, paren).trim().substringBefore(' ').removeSurrounding("<", ">")
                            b.withLink(LinkAnnotation.Url(target, TextLinkStyles(style.link))) {
                                emit(b, s.substring(i + 1, close), style)
                            }
                            i = paren + 1
                            continue
                        }
                    }
                    plain.append(c)
                    i++
                }
                c == '<' && (s.startsWith("<http://", i) || s.startsWith("<https://", i)) && s.indexOf('>', i) > 0 -> {
                    flush()
                    val end = s.indexOf('>', i)
                    val target = s.substring(i + 1, end)
                    b.withLink(LinkAnnotation.Url(target, TextLinkStyles(style.link))) { append(target) }
                    i = end + 1
                }
                c == 'h' && (s.startsWith("http://", i) || s.startsWith("https://", i)) && (i == 0 || !s[i - 1].isLetterOrDigit()) -> {
                    val m = url.find(s, i)
                    if (m != null && m.range.first == i) {
                        flush()
                        b.withLink(LinkAnnotation.Url(m.value, TextLinkStyles(style.link))) { append(m.value) }
                        i = m.range.last + 1
                    } else {
                        plain.append(c)
                        i++
                    }
                }
                else -> {
                    plain.append(c)
                    i++
                }
            }
        }
        flush()
    }

    private fun emphasisOpens(s: String, i: Int): Boolean {
        val next = s.getOrNull(i + 1) ?: return false
        if (next.isWhitespace()) return false
        // `_` inside a word (snake_case) is not emphasis.
        return !(s[i] == '_' && i > 0 && s[i - 1].isLetterOrDigit())
    }

    private fun emphasisCloses(s: String, from: Int, c: Char): Int {
        var j = from
        while (j < s.length) {
            if (s[j] == '`') {
                val end = s.indexOf('`', j + 1)
                if (end < 0) return -1
                j = end + 1
                continue
            }
            if (s[j] == c && j > from && !s[j - 1].isWhitespace() &&
                !(c == '_' && s.getOrNull(j + 1)?.isLetterOrDigit() == true) &&
                s.getOrNull(j + 1) != c
            ) {
                return j
            }
            j++
        }
        return -1
    }

    private fun matching(s: String, open: Int, a: Char, z: Char): Int {
        var depth = 0
        var j = open
        while (j < s.length) {
            when (s[j]) {
                '\\' -> j++
                a -> depth++
                z -> {
                    depth--
                    if (depth == 0) return j
                }
            }
            j++
        }
        return -1
    }
}
