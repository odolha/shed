package dev.shed.ui.ai

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Build
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.ErrorOutline
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Psychology
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Public
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shed.R
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.material.icons.filled.Settings
import androidx.compose.ui.draw.drawBehind
import dev.shed.api.AiActivity
import dev.shed.api.AiEntry
import dev.shed.api.AiNote
import dev.shed.api.AiNoteKind
import dev.shed.api.AiRun
import dev.shed.api.AiRunState
import dev.shed.api.AiStatus
import dev.shed.ui.common.Chip
import dev.shed.api.AiPermission
import dev.shed.api.AiQuestion
import dev.shed.api.AiTextEdit
import dev.shed.api.AiTool
import dev.shed.api.AiUser
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.clock
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono

/** One of your messages. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun UserEntry(
    user: AiUser,
    pictures: Map<String, ImageBitmap?>,
    loadPicture: (String) -> Unit,
    onLongPress: () -> Unit,
    onPicture: (ImageBitmap) -> Unit,
) {
    val shed = LocalShed.current
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        Column(
            Modifier
                .widthIn(max = 560.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(shed.accent.copy(alpha = 0.14f))
                .combinedClickable(onClick = {}, onLongClick = onLongPress)
                .padding(12.dp),
        ) {
            Text(user.text, style = MaterialTheme.typography.bodyMedium)
            val names = user.pictures.orEmpty()
            if (names.isNotEmpty()) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.padding(top = 6.dp)) {
                    for (name in names) Thumb(name, pictures, loadPicture, onPicture)
                }
            }
            val missing = (user.images?.toInt() ?: 0) - names.size
            if (missing > 0) {
                Text(pluralStringResource(R.plurals.ai_pictures_missing, missing, missing), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
            }
        }
        user.at?.let { at ->
            Text(clock(at.toLong()), color = shed.textDim, fontSize = 10.sp, modifier = Modifier.padding(top = 2.dp, end = 4.dp))
        }
    }
}

@Composable
fun Thumb(name: String, pictures: Map<String, ImageBitmap?>, load: (String) -> Unit, onOpen: (ImageBitmap) -> Unit) {
    val shed = LocalShed.current
    androidx.compose.runtime.LaunchedEffect(name) { load(name) }
    val bitmap = pictures[name]
    Box(
        Modifier.size(72.dp).clip(RoundedCornerShape(6.dp)).background(shed.hover).then(
            if (bitmap != null) Modifier.clickable { onOpen(bitmap) } else Modifier,
        ),
        contentAlignment = Alignment.Center,
    ) {
        if (bitmap != null) {
            Image(bitmap, null, contentScale = ContentScale.Crop, modifier = Modifier.size(72.dp))
        } else if (name !in pictures) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        } else {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        }
    }
}

/** Claude's thinking, folded away. */
@Composable
fun ThinkingEntry(text: String) {
    val shed = LocalShed.current
    var open by rememberSaveable(text.hashCode()) { mutableStateOf(false) }
    Column {
        Row(
            Modifier.clickable { open = !open }.padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Psychology, null, tint = shed.textDim, modifier = Modifier.size(18.dp))
            HSpace(6)
            Text(stringResource(R.string.ai_thinking), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
            Icon(if (open) Icons.Default.ExpandMore else Icons.Default.ChevronRight, null, tint = shed.textDim, modifier = Modifier.size(18.dp))
        }
        if (open) Text(text, color = shed.textDim, fontStyle = FontStyle.Italic, style = MaterialTheme.typography.bodySmall)
    }
}

private fun toolIcon(name: String): ImageVector = when {
    name == "Bash" -> Icons.Default.Terminal
    name in setOf("Edit", "Write", "MultiEdit", "NotebookEdit") -> Icons.Default.Edit
    name in setOf("Read") -> Icons.Default.Description
    name in setOf("Grep", "Glob", "WebSearch") -> Icons.Default.Search
    name.startsWith("mcp__claude-in-chrome") || name == "WebFetch" -> Icons.Default.Public
    else -> Icons.Default.Build
}

/**
 * A step Claude took, on one line; open for its input, change and result. A
 * sub-agent or a task in the background shows how it is doing after it, and
 * opens to the agent's own steps and report; a step that set a timer, when it fires.
 */
@Composable
fun ToolEntry(
    tool: AiTool,
    status: AiStatus?,
    now: () -> Long,
    pictures: Map<String, ImageBitmap?>,
    loadPicture: (String) -> Unit,
    onPicture: (ImageBitmap) -> Unit,
) {
    val shed = LocalShed.current
    var open by rememberSaveable(tool.id) { mutableStateOf(false) }
    val run = tool.run
    val working = status != null && status.state != AiActivity.Idle
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).clickable { open = !open }.padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                if (tool.error == true) Icons.Default.ErrorOutline else toolIcon(tool.name),
                null,
                tint = if (tool.error == true) shed.danger else shed.textDim,
                modifier = Modifier.size(16.dp),
            )
            HSpace(8)
            Column(Modifier.weight(1f)) {
                Text(
                    tool.summary,
                    fontFamily = Mono,
                    fontSize = 12.sp,
                    color = shed.text,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                // How a run is doing; when a timer it set fires.
                val info = if (run != null) runLine(tool, run, now()) else firesLine(tool, status, now())
                if (info != null) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        if (run?.background == true) {
                            Chip(stringResource(R.string.ai_background), shed.textDim)
                            HSpace(6)
                        }
                        Text(
                            info,
                            color = if (run?.running == true) shed.ink(shed.accent) else shed.textDim,
                            fontSize = 11.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
            HSpace(6)
            when {
                run != null -> RunMark(run.stateOrRunning, working)
                tool.result == null -> CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
            }
        }
        if (open && run != null && tool.isAgent) {
            AgentView(tool, run, working)
        } else if (open) {
            Column(Modifier.padding(start = 24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                tool.detail?.takeIf { it.isNotBlank() }?.let { CodeBox(it) }
                tool.edit?.let { EditDiff(it) }
                tool.result?.takeIf { it.isNotBlank() }?.let {
                    CodeBox(it, if (tool.error == true) shed.danger else shed.textDim)
                }
                val names = tool.pictures.orEmpty()
                if (names.isNotEmpty()) {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        for (name in names) Thumb(name, pictures, loadPicture, onPicture)
                    }
                }
            }
        }
    }
}

/** A run's mark: turning while it goes on (still while Claude is idle), ✓ done, ✗ failed, ■ stopped. */
@Composable
private fun RunMark(state: AiRunState, working: Boolean) {
    val shed = LocalShed.current
    when (state) {
        AiRunState.Running -> if (working) {
            CircularProgressIndicator(Modifier.size(12.dp), color = shed.accent, strokeWidth = 1.5.dp)
        } else {
            Mark("◐", shed.ink(shed.accent))
        }
        AiRunState.Done -> Mark("✓", shed.ink(shed.accent))
        AiRunState.Failed -> Mark("✗", shed.danger)
        AiRunState.Stopped -> Mark("■", shed.textDim)
    }
}

@Composable
private fun Mark(text: String, color: androidx.compose.ui.graphics.Color) {
    Text(text, color = color, fontSize = 12.sp, textAlign = TextAlign.Center, modifier = Modifier.widthIn(min = 12.dp))
}

/** An opened sub-agent: its prompt, kind and tokens, its steps (tool calls that open, what it said), its report. */
@Composable
private fun AgentView(tool: AiTool, run: AiRun, working: Boolean) {
    val shed = LocalShed.current
    Column(Modifier.padding(start = 24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        tool.detail?.takeIf { it.isNotBlank() }?.let { CodeBox(it) }
        agentLine(run).takeIf { it.isNotEmpty() }?.let {
            Text(it, color = shed.textDim, style = MaterialTheme.typography.bodySmall)
        }
        Column(
            Modifier
                .fillMaxWidth()
                .drawBehind { drawRect(shed.divider, size = androidx.compose.ui.geometry.Size(2.dp.toPx(), size.height)) }
                .padding(start = 10.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            val skipped = run.skipped?.toInt() ?: 0
            if (skipped > 0) {
                Text(pluralStringResource(R.plurals.ai_steps_skipped, skipped, skipped), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
            }
            val steps = run.steps.orEmpty()
            if (steps.isEmpty()) {
                Text(
                    stringResource(if (run.running) R.string.ai_no_steps_yet else R.string.ai_steps_unknown),
                    color = shed.textDim,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            for (step in steps) {
                when (step) {
                    is AiEntry.Tool -> AgentStep(tool.id, step.content, run.running, working)
                    is AiEntry.Assistant -> if (step.content.text.isNotBlank()) MarkdownView(step.content.text)
                    else -> {}
                }
            }
        }
        val report = runReport(tool, run)
        if (report.isNotBlank()) {
            Text(stringResource(R.string.ai_its_report), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
            MarkdownView(report)
        }
    }
}

/** One of a sub-agent's tool calls: a line that opens to its change or input, and what it returned. */
@Composable
private fun AgentStep(parent: String, step: AiTool, running: Boolean, working: Boolean) {
    val shed = LocalShed.current
    var open by rememberSaveable(parent + "/" + step.id) { mutableStateOf(false) }
    val more = step.result != null || step.edit != null || !step.detail.isNullOrBlank()
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(6.dp))
            .then(if (more) Modifier.clickable { open = !open } else Modifier)
            .padding(vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when {
            step.result == null && running -> RunMark(AiRunState.Running, working)
            step.result == null -> Mark("○", shed.textDim)
            step.error == true -> Mark("✗", shed.danger)
            else -> Mark("✓", shed.ink(shed.accent))
        }
        HSpace(8)
        Text(
            step.summary,
            fontFamily = Mono,
            fontSize = 12.sp,
            color = shed.textDim,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (more) Icon(if (open) Icons.Default.ExpandMore else Icons.Default.ChevronRight, null, tint = shed.textDim, modifier = Modifier.size(16.dp))
    }
    if (open) {
        Column(Modifier.padding(start = 20.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            val edit = step.edit
            if (edit != null) EditDiff(edit) else step.detail?.takeIf { it.isNotBlank() }?.let { CodeBox(it) }
            step.result?.takeIf { it.isNotBlank() }?.let { CodeBox(it, if (step.error == true) shed.danger else shed.textDim) }
        }
    }
}

/** One of Claude Code's own notes: a line with its kind's icon and time, its detail folded. */
@Composable
fun NoteEntry(note: AiNote) {
    val shed = LocalShed.current
    val detail = note.detail.orEmpty()
    val foldable = detail.isNotBlank()
    var open by rememberSaveable(note.title, note.at?.toLong() ?: 0L) { mutableStateOf(false) }
    val error = note.error == true
    val kind = note.kind ?: AiNoteKind.Task
    val (icon, color) = when {
        error -> Icons.Default.ErrorOutline to shed.danger
        kind == AiNoteKind.Wakeup -> Icons.Default.Schedule to shed.ink(shed.accent)
        kind == AiNoteKind.Command -> Icons.Default.Terminal to shed.textDim
        kind == AiNoteKind.Compacted -> Icons.Default.History to shed.textDim
        kind == AiNoteKind.Message -> Icons.AutoMirrored.Filled.Send to shed.textDim
        kind == AiNoteKind.Hook -> Icons.Default.Settings to shed.textDim
        else -> Icons.Default.Build to shed.textDim
    }
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(6.dp))
                .then(if (foldable) Modifier.clickable { open = !open } else Modifier)
                .padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(icon, null, tint = color, modifier = Modifier.size(16.dp))
            HSpace(8)
            Text(
                note.title,
                color = if (error) shed.danger else shed.textDim,
                style = MaterialTheme.typography.bodySmall,
                maxLines = if (open) Int.MAX_VALUE else 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            note.at?.toLong()?.takeIf { it > 0 }?.let {
                HSpace(6)
                Text(clock(it), color = shed.textDim, fontSize = 10.sp)
            }
            if (foldable) Icon(if (open) Icons.Default.ExpandMore else Icons.Default.ChevronRight, null, tint = shed.textDim, modifier = Modifier.size(16.dp))
        }
        if (open && foldable) {
            Column(Modifier.padding(start = 24.dp, top = 2.dp)) {
                // A command's output and a hook's context are text as it came; the rest Markdown.
                if (kind == AiNoteKind.Command || kind == AiNoteKind.Hook) CodeBox(detail, shed.textDim) else MarkdownView(detail)
            }
        }
    }
}

@Composable
fun CodeBox(text: String, color: androidx.compose.ui.graphics.Color = LocalShed.current.text) {
    val shed = LocalShed.current
    Text(
        text,
        fontFamily = Mono,
        fontSize = 11.sp,
        color = color,
        softWrap = false,
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(max = 320.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(shed.surface)
            .border(1.dp, shed.divider, RoundedCornerShape(6.dp))
            .horizontalScroll(rememberScrollState())
            .padding(8.dp),
    )
}

/** An edit's text, as removed and added lines. */
@Composable
fun EditDiff(edit: AiTextEdit) {
    val shed = LocalShed.current
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(6.dp))
            .border(1.dp, shed.divider, RoundedCornerShape(6.dp))
            .horizontalScroll(rememberScrollState()),
    ) {
        if (edit.old.isNotEmpty()) {
            for (line in edit.old.split('\n')) {
                Text("− $line", fontFamily = Mono, fontSize = 11.sp, softWrap = false, modifier = Modifier.background(shed.removedBackground).padding(horizontal = 6.dp))
            }
        }
        for (line in edit.new.split('\n')) {
            Text("+ $line", fontFamily = Mono, fontSize = 11.sp, softWrap = false, modifier = Modifier.background(shed.addedBackground).padding(horizontal = 6.dp))
        }
    }
}

/** Claude asks to use a tool (Allow / Allow for this session / Deny), or asks you questions. */
@Composable
fun PermissionEntry(
    permission: AiPermission,
    canAnswer: Boolean,
    onAnswer: (allow: Boolean, always: Boolean) -> Unit,
    onAnswers: (List<String>) -> Unit,
) {
    val shed = LocalShed.current
    val waiting = permission.answer == null && permission.answers.isNullOrEmpty()
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .border(1.dp, if (waiting) shed.warn else shed.divider, RoundedCornerShape(10.dp))
            .background(shed.surface)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val questions = permission.questions.orEmpty()
        if (questions.isNotEmpty()) {
            Questions(questions, permission.answers.orEmpty(), waiting && canAnswer, onAnswers) { onAnswer(false, false) }
            return@Column
        }
        Text(
            if (waiting) stringResource(R.string.ai_permission_asks, permission.tool) else permission.tool,
            fontWeight = FontWeight.Medium,
            color = if (waiting) shed.warn else shed.textDim,
            style = MaterialTheme.typography.labelLarge,
        )
        Text(permission.summary, fontFamily = Mono, fontSize = 12.sp)
        permission.detail?.takeIf { it.isNotBlank() && it != permission.summary }?.let { CodeBox(it) }
        permission.edit?.let { EditDiff(it) }
        when {
            permission.answer == true -> Text(stringResource(R.string.ai_allowed), color = shed.good, style = MaterialTheme.typography.bodySmall)
            permission.answer == false -> Text(stringResource(R.string.ai_denied), color = shed.danger, style = MaterialTheme.typography.bodySmall)
            canAnswer -> FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onAnswer(true, false) }) { Text(stringResource(R.string.ai_allow)) }
                OutlinedButton(onClick = { onAnswer(true, true) }) { Text(stringResource(R.string.ai_allow_session)) }
                TextButton(
                    onClick = { onAnswer(false, false) },
                    colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                ) { Text(stringResource(R.string.ai_deny)) }
            }
        }
    }
}

@Composable
private fun Questions(
    questions: List<AiQuestion>,
    answers: List<String>,
    canAnswer: Boolean,
    onAnswers: (List<String>) -> Unit,
    onSkip: () -> Unit,
) {
    val shed = LocalShed.current
    val picks = remember(questions) { questions.map { mutableStateListOf<String>() } }
    val others = remember(questions) { mutableStateListOf(*Array(questions.size) { "" }) }
    fun answerOf(i: Int): String = (picks[i] + listOfNotNull(others[i].trim().takeIf { it.isNotEmpty() })).joinToString(", ")
    questions.forEachIndexed { qi, q ->
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (q.header.isNotBlank()) Text(q.header.uppercase(), style = MaterialTheme.typography.labelSmall, color = shed.textDim)
            Text(q.question, fontWeight = FontWeight.Medium)
            if (!canAnswer) {
                answers.getOrNull(qi)?.let { Text("→ $it", color = shed.accent) }
                return@Column
            }
            for (option in q.options) {
                Row(
                    Modifier.fillMaxWidth().clickable {
                        if (q.multi == true) {
                            if (option.label in picks[qi]) picks[qi].remove(option.label) else picks[qi].add(option.label)
                        } else {
                            picks[qi].clear()
                            picks[qi].add(option.label)
                            others[qi] = ""
                            if (questions.size == 1) onAnswers(listOf(option.label))
                        }
                    },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (q.multi == true) {
                        Checkbox(option.label in picks[qi], null)
                    } else {
                        RadioButton(option.label in picks[qi], null)
                    }
                    Column(Modifier.padding(start = 4.dp, top = 6.dp, bottom = 6.dp)) {
                        Text(option.label)
                        option.description?.takeIf { it.isNotBlank() }?.let {
                            Text(it, color = shed.textDim, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
            OutlinedTextField(
                value = others[qi],
                onValueChange = {
                    others[qi] = it
                    if (q.multi != true && it.isNotEmpty()) picks[qi].clear()
                },
                placeholder = { Text(stringResource(R.string.ai_other_answer)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
    if (canAnswer) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(
                enabled = questions.indices.all { answerOf(it).isNotEmpty() },
                onClick = { onAnswers(questions.indices.map { answerOf(it) }) },
            ) { Text(stringResource(R.string.ai_send_answers)) }
            TextButton(onClick = onSkip) { Text(stringResource(R.string.ai_skip)) }
        }
    }
}

/** A finished turn, an interruption, an error. */
@Composable
fun NoticeEntry(text: String, error: Boolean) {
    val shed = LocalShed.current
    Text(
        text,
        color = if (error) shed.danger else shed.textDim,
        style = MaterialTheme.typography.bodySmall,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
    )
}
