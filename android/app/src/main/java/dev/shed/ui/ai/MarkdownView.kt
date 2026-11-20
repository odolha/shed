package dev.shed.ui.ai

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shed.R
import dev.shed.ui.logbook.copy
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono

/** Claude's Markdown, laid out natively and selectable. */
@Composable
fun MarkdownView(text: String, modifier: Modifier = Modifier) {
    val shed = LocalShed.current
    val style = remember(shed) {
        MdStyle(
            code = SpanStyle(fontFamily = Mono, background = shed.text.copy(alpha = 0.08f), fontSize = 13.sp),
            link = SpanStyle(color = shed.accent, textDecoration = TextDecoration.Underline),
        )
    }
    val blocks = remember(text, style) { Markdown.parse(text, style) }
    SelectionContainer(modifier) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (block in blocks) Block(block)
        }
    }
}

@Composable
private fun Block(block: MdBlock) {
    val shed = LocalShed.current
    val context = LocalContext.current
    when (block) {
        is MdBlock.Paragraph -> Text(block.text, style = MaterialTheme.typography.bodyMedium)
        is MdBlock.Heading -> Text(
            block.text,
            style = when (block.level) {
                1 -> MaterialTheme.typography.titleLarge
                2 -> MaterialTheme.typography.titleMedium
                else -> MaterialTheme.typography.titleSmall
            },
            fontWeight = FontWeight.SemiBold,
        )
        is MdBlock.Code -> Box(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(6.dp))
                .background(shed.surface)
                .border(1.dp, shed.divider, RoundedCornerShape(6.dp)),
        ) {
            Text(
                block.code,
                fontFamily = Mono,
                fontSize = 12.sp,
                softWrap = false,
                modifier = Modifier.horizontalScroll(rememberScrollState()).padding(start = 10.dp, top = 10.dp, bottom = 10.dp, end = 40.dp),
            )
            IconButton(onClick = { copy(context, block.code) }, modifier = Modifier.align(androidx.compose.ui.Alignment.TopEnd)) {
                Icon(Icons.Default.ContentCopy, stringResource(R.string.copy), tint = shed.textDim)
            }
        }
        is MdBlock.Quote -> Row(Modifier.height(IntrinsicSize.Min)) {
            Box(Modifier.width(3.dp).fillMaxHeight().background(shed.divider))
            Column(Modifier.padding(start = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                for (b in block.blocks) Block(b)
            }
        }
        is MdBlock.ListBlock -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEachIndexed { index, item ->
                Row {
                    Text(
                        if (block.ordered) "${block.start + index}." else "•",
                        color = shed.textDim,
                        modifier = Modifier.widthIn(min = 20.dp).padding(end = 6.dp),
                    )
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) { for (b in item) Block(b) }
                }
            }
        }
        is MdBlock.Table -> Column(
            Modifier.horizontalScroll(rememberScrollState()).border(1.dp, shed.divider, RoundedCornerShape(4.dp)),
        ) {
            val columns = (listOf(block.header) + block.rows).maxOf { it.size }
            for ((r, row) in (listOf(block.header) + block.rows).withIndex()) {
                Row(if (r == 0) Modifier.background(shed.surface) else Modifier) {
                    for (c in 0 until columns) {
                        Text(
                            row.getOrNull(c) ?: androidx.compose.ui.text.AnnotatedString(""),
                            fontWeight = if (r == 0) FontWeight.SemiBold else null,
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.widthIn(min = 60.dp, max = 240.dp).padding(6.dp),
                        )
                    }
                }
                if (r == 0) HorizontalDivider(color = shed.divider)
            }
        }
        MdBlock.Rule -> HorizontalDivider(color = shed.divider)
    }
}
