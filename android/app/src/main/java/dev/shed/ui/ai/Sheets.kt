package dev.shed.ui.ai

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.SubdirectoryArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.EditNote
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.shed.R
import dev.shed.api.AiDraft
import dev.shed.api.AiOpenSession
import dev.shed.api.AiRemoveDraft
import dev.shed.api.AiRenameSession
import dev.shed.api.AiSend
import dev.shed.api.AiSession
import dev.shed.api.AiSetUnsent
import dev.shed.api.AiTab
import dev.shed.api.Call
import dev.shed.ui.LocalSession
import dev.shed.ui.common.ConfirmDialog
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.Loading
import dev.shed.ui.common.TextDialog
import dev.shed.ui.common.ago
import dev.shed.ui.common.rememberRunner
import dev.shed.ui.theme.LocalShed

/** The folder's sessions, newest first, with branches and draft sessions; open, start new, rename. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun SessionsSheet(
    ws: String,
    tab: String,
    vm: AiVm,
    sessions: List<AiSession>?,
    current: String?,
    canChange: Boolean,
    onDismiss: () -> Unit,
) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val context = LocalContext.current
    val runner = rememberRunner()
    var renaming by remember { mutableStateOf<AiSession?>(null) }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Text(stringResource(R.string.ai_sessions), style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = 16.dp))
        if (canChange) {
            TextButton(
                onClick = {
                    onDismiss()
                    runner.run { session.socket.call(ws, Call.AiOpenSession(AiOpenSession(tab, null))) }
                },
                modifier = Modifier.padding(horizontal = 8.dp),
            ) { Icon(Icons.Default.Add, null); HSpace(6); Text(stringResource(R.string.ai_new_session)) }
        }
        val list = sessions
        if (list == null) {
            Loading(Modifier.padding(32.dp))
        } else {
            LazyColumn(contentPadding = PaddingValues(bottom = 32.dp)) {
                items(list, key = { it.id }) { s ->
                    val depth = s.depth?.toInt() ?: 0
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .background(if (s.id == current) shed.accent.copy(alpha = 0.12f) else Color.Transparent)
                            .combinedClickable(
                                onClick = {
                                    onDismiss()
                                    if (canChange) runner.run { session.socket.call(ws, Call.AiOpenSession(AiOpenSession(tab, s.id))) }
                                },
                                onLongClick = { if (canChange && s.draft != true) renaming = s },
                            )
                            .padding(start = (16 + depth * 16).dp, end = 16.dp, top = 8.dp, bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        when {
                            s.draft == true -> Icon(Icons.Default.EditNote, null, tint = shed.accent, modifier = Modifier.size(18.dp))
                            depth > 0 -> Icon(Icons.Default.SubdirectoryArrowRight, null, tint = shed.textDim, modifier = Modifier.size(18.dp))
                        }
                        if (s.draft == true || depth > 0) HSpace(8)
                        Column(Modifier.weight(1f)) {
                            Text(s.title, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            val parts = buildList {
                                add(ago(context, s.modified.toLong()))
                                s.branch?.let { add(it) }
                                if (s.draft == true) {
                                    add(context.resources.getQuantityString(R.plurals.ai_drafts_kept, s.drafts?.toInt() ?: 0, s.drafts?.toInt() ?: 0))
                                } else {
                                    add(context.resources.getQuantityString(R.plurals.ai_prompts, s.prompts.toInt(), s.prompts.toInt()))
                                    val d = s.drafts?.toInt() ?: 0
                                    if (d > 0) add(context.resources.getQuantityString(R.plurals.ai_drafts_kept, d, d))
                                }
                            }
                            Text(parts.joinToString(" · "), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        }
    }
    renaming?.let { s ->
        TextDialog(
            title = stringResource(R.string.ai_rename_session),
            initial = s.title,
            confirm = stringResource(R.string.rename),
            allowEmpty = true,
            onConfirm = { name ->
                runner.run {
                    session.socket.call(ws, Call.AiRenameSession(AiRenameSession(tab, s.id, name)))
                    vm.loadSessions()
                }
            },
            onDismiss = { renaming = null },
        )
    }
}

/** The drafts of the session shown: send, edit (back into the prompt), delete. */
@Composable
fun DraftsSheet(ws: String, tab: String, ai: AiTab, current: String?, canChange: Boolean, onDismiss: () -> Unit) {
    val session = LocalSession.current
    val shed = LocalShed.current
    val runner = rememberRunner()
    var deleting by remember { mutableStateOf<Pair<Int, AiDraft>?>(null) }
    val drafts = ai.drafts.orEmpty().withIndex().filter { it.value.session == current }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Text(stringResource(R.string.ai_drafts), style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = 16.dp))
        if (drafts.isEmpty()) {
            Text(stringResource(R.string.ai_no_drafts), color = shed.textDim, modifier = Modifier.padding(16.dp))
        }
        LazyColumn(contentPadding = PaddingValues(bottom = 32.dp)) {
            items(drafts, key = { it.index }) { (index, draft) ->
                Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
                    Text(draft.text, maxLines = 6, overflow = TextOverflow.Ellipsis)
                    if (canChange) {
                        Row {
                            TextButton(onClick = {
                                onDismiss()
                                runner.run {
                                    session.socket.call(ws, Call.AiSend(AiSend(tab, draft.text, null)))
                                    session.socket.call(ws, Call.AiRemoveDraft(AiRemoveDraft(tab, index.toUInt(), draft.text)))
                                }
                            }) { Icon(Icons.AutoMirrored.Filled.Send, null); HSpace(6); Text(stringResource(R.string.send)) }
                            TextButton(onClick = {
                                onDismiss()
                                runner.run {
                                    session.socket.call(ws, Call.AiSetUnsent(AiSetUnsent(tab, draft.text)))
                                    session.socket.call(ws, Call.AiRemoveDraft(AiRemoveDraft(tab, index.toUInt(), draft.text)))
                                }
                            }) { Icon(Icons.Default.Edit, null); HSpace(6); Text(stringResource(R.string.ai_edit)) }
                            TextButton(
                                onClick = { deleting = index to draft },
                                colors = ButtonDefaults.textButtonColors(contentColor = shed.danger),
                            ) { Text(stringResource(R.string.delete_ellipsis)) }
                        }
                    }
                }
            }
        }
    }
    deleting?.let { (index, draft) ->
        ConfirmDialog(
            title = stringResource(R.string.ai_delete_draft),
            text = draft.text.take(200),
            confirm = stringResource(R.string.delete),
            onConfirm = { runner.run { session.socket.call(ws, Call.AiRemoveDraft(AiRemoveDraft(tab, index.toUInt(), draft.text))) } },
            onDismiss = { deleting = null },
        )
    }
}
