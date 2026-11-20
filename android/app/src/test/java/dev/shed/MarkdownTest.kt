package dev.shed

import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import dev.shed.ui.ai.Markdown
import dev.shed.ui.ai.MdBlock
import dev.shed.ui.devices.normalizePairCode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MarkdownTest {
    @Test
    fun blocks() {
        val md = """
            # Plan

            Some **bold** and `code`
            on two lines.

            - one
            - two
              - nested

            1. first
            2. second

            ```kotlin
            val x = 1
            ```

            > quoted

            | a | b |
            |---|---|
            | 1 | 2 |

            ---
        """.trimIndent()
        val blocks = Markdown.parse(md)
        assertEquals(
            listOf(
                MdBlock.Heading::class, MdBlock.Paragraph::class, MdBlock.ListBlock::class, MdBlock.ListBlock::class,
                MdBlock.Code::class, MdBlock.Quote::class, MdBlock.Table::class, MdBlock.Rule::class,
            ),
            blocks.map { it::class },
        )
        val para = (blocks[1] as MdBlock.Paragraph).text
        assertEquals("Some bold and code on two lines.", para.text)
        val bold = para.spanStyles.single { it.item.fontWeight == FontWeight.Bold }
        assertEquals("bold", para.text.substring(bold.start, bold.end))

        val list = blocks[2] as MdBlock.ListBlock
        assertEquals(false, list.ordered)
        assertEquals(2, list.items.size)
        assertTrue(list.items[1].any { it is MdBlock.ListBlock })
        val ordered = blocks[3] as MdBlock.ListBlock
        assertEquals(true, ordered.ordered)
        assertEquals(1, ordered.start)

        val code = blocks[4] as MdBlock.Code
        assertEquals("kotlin", code.lang)
        assertEquals("val x = 1", code.code)
        val table = blocks[6] as MdBlock.Table
        assertEquals(listOf("a", "b"), table.header.map { it.text })
        assertEquals(listOf("1", "2"), table.rows.single().map { it.text })
    }

    @Test
    fun inline() {
        val text = Markdown.inline("an *italic*, a snake_case_name, [a link](https://example.com) and ~~gone~~")
        assertEquals("an italic, a snake_case_name, a link and gone", text.text)
        val italic = text.spanStyles.single { it.item.fontStyle == FontStyle.Italic }
        assertEquals("italic", text.text.substring(italic.start, italic.end))
        val link = text.getLinkAnnotations(0, text.length).single()
        assertEquals("https://example.com", (link.item as LinkAnnotation.Url).url)
        assertEquals("a link", text.text.substring(link.start, link.end))
    }

    @Test
    fun bareUrlsAreLinks() {
        val text = Markdown.inline("see https://example.com/docs.")
        val link = text.getLinkAnnotations(0, text.length).single()
        assertEquals("https://example.com/docs", (link.item as LinkAnnotation.Url).url)
    }

    @Test
    fun pairingCodesAsTyped() {
        assertEquals("K7QF-2M9D", normalizePairCode("k7qf2m9d"))
        assertEquals("K7QF-2M9D", normalizePairCode(" k7qf-2m9d "))
    }
}
