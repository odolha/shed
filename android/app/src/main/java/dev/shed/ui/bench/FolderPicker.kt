package dev.shed.ui.bench

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.shed.R
import dev.shed.api.Browse
import dev.shed.api.Browsed
import dev.shed.api.Call
import dev.shed.api.ErrorCode
import dev.shed.api.FolderEntry
import dev.shed.api.Reply
import dev.shed.net.ApiException
import dev.shed.net.NexusSocket
import dev.shed.ui.LocalSession
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.errorText
import dev.shed.ui.theme.LocalShed

/**
 * Chooses a folder on the workspace (`Browse`), starting at [start]: shows
 * where it is as an absolute path and goes up through the folder above.
 */
@Composable
fun FolderPicker(
    ws: String,
    start: String,
    home: String,
    title: String,
    onPick: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val socket = LocalSession.current.socket
    val context = LocalContext.current
    val shed = LocalShed.current
    var path by rememberSaveable { mutableStateOf(start) }
    var shown by remember { mutableStateOf<Browsed?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(path) {
        error = null
        shown = try {
            browse(socket, ws, path, home)
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
            worked(path, home, emptyList())
        }
    }
    val here = shown
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                Text(
                    here?.path ?: path.ifEmpty { home },
                    style = MaterialTheme.typography.bodySmall,
                    color = shed.textDim,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                error?.let { Text(it, color = shed.danger) }
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 380.dp)) {
                    here?.parent?.let { parent ->
                        item { FolderRow(Icons.Default.ArrowUpward, "..") { path = parent } }
                    }
                    items(here?.entries.orEmpty().filter { it.dir }, key = { it.name }) { entry ->
                        FolderRow(Icons.Default.Folder, entry.name, dim = entry.ignored == true) {
                            path = here!!.path.trimEnd('/') + "/" + entry.name
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                enabled = here != null && error == null,
                onClick = {
                    onDismiss()
                    onPick(here!!.path)
                },
            ) { Text(stringResource(R.string.folder_use_this)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/**
 * A folder's folders, its absolute path and the folder above. A workspace
 * whose Shed predates `Browse` answers `BrowseFolders` only: the path is then
 * worked out here.
 */
private suspend fun browse(socket: NexusSocket, ws: String, path: String, home: String): Browsed {
    try {
        val reply = socket.call(ws, Call.Browse(Browse(path)))
        return (reply as? Reply.Browsed)?.content ?: Browsed(path, null, emptyList())
    } catch (e: ApiException) {
        if (e.code != ErrorCode.Unsupported) throw e
    }
    val reply = socket.call(ws, Call.BrowseFolders(Browse(path)))
    return worked(path, home, (reply as? Reply.Folder)?.content.orEmpty())
}

/** [path] made absolute here (`~`, `""`: [home]) with the folder above. */
private fun worked(path: String, home: String, entries: List<FolderEntry>): Browsed {
    val absolute = when {
        path.isEmpty() || path == "~" -> home
        path.startsWith("~/") -> home.trimEnd('/') + path.removePrefix("~")
        else -> path
    }.ifEmpty { "/" }
    val parent = if (absolute == "/") null else absolute.trimEnd('/').substringBeforeLast('/', "").ifEmpty { "/" }
    return Browsed(absolute, parent, entries)
}

@Composable
private fun FolderRow(icon: androidx.compose.ui.graphics.vector.ImageVector, name: String, dim: Boolean = false, onClick: () -> Unit) {
    val shed = LocalShed.current
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, tint = if (dim) shed.textDim else shed.accent)
        HSpace(12)
        Text(name, color = if (dim) shed.textDim else shed.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}
