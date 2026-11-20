package dev.shed.ui.ai

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.navigation.NavController
import dev.shed.R
import dev.shed.api.AiExplain
import dev.shed.api.AiSetViewed
import dev.shed.api.Call
import dev.shed.api.ErrorCode
import dev.shed.api.Reply
import dev.shed.api.Review
import dev.shed.api.ReviewChange
import dev.shed.api.TabOnly
import dev.shed.net.ApiException
import dev.shed.ui.LocalSession
import dev.shed.ui.canChange
import dev.shed.ui.common.DiffView
import dev.shed.ui.common.EmptyNote
import dev.shed.ui.common.HSpace
import dev.shed.ui.common.Loading
import dev.shed.ui.common.LocalSnackbar
import dev.shed.ui.common.SectionTitle
import dev.shed.ui.common.ShedScaffold
import dev.shed.ui.common.errorText
import dev.shed.ui.common.tildePath
import dev.shed.ui.git.statusColor
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono
import dev.shed.ui.workspaceState
import kotlinx.coroutines.launch

/** What Claude changed in the tab's repositories: diffs, Viewed marks, explanations. */
@Composable
fun ReviewScreen(nav: NavController, ws: String, tab: String) {
    val session = LocalSession.current
    val context = LocalContext.current
    val shed = LocalShed.current
    val snackbar = LocalSnackbar.current
    val scope = rememberCoroutineScope()
    val home = workspaceState(ws)?.home.orEmpty()
    val canChange = canChange(ws)
    var refresh by remember { mutableIntStateOf(0) }
    var review by remember { mutableStateOf<Review?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val open = remember { mutableStateMapOf<String, Boolean>() }
    val explained = remember { mutableStateMapOf<String, String>() }
    val explaining = remember { mutableStateMapOf<String, Boolean>() }
    val unsupported = stringResource(R.string.review_explain_unsupported)

    LaunchedEffect(refresh) {
        try {
            review = (session.socket.call(ws, Call.AiReview(TabOnly(tab)), 60_000) as? Reply.Review)?.content
            error = null
        } catch (e: Exception) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            error = errorText(context, e)
        }
    }

    fun explain(keys: List<String>) {
        keys.forEach { explaining[it] = true }
        scope.launch {
            try {
                val reply = session.socket.call(ws, Call.AiExplain(AiExplain(tab, keys)), 180_000)
                (reply as? Reply.Explained)?.content?.forEach { explained[it.key] = it.text }
            } catch (e: Exception) {
                if (e is kotlinx.coroutines.CancellationException) throw e
                snackbar.showSnackbar(if (e is ApiException && e.code == ErrorCode.Unsupported) unsupported else errorText(context, e))
            } finally {
                keys.forEach { explaining.remove(it) }
            }
        }
    }

    fun setViewed(change: ReviewChange, viewed: Boolean) {
        scope.launch {
            try {
                session.socket.call(ws, Call.AiSetViewed(AiSetViewed(tab, change.key, viewed)))
                refresh++
            } catch (e: Exception) {
                if (e is kotlinx.coroutines.CancellationException) throw e
                snackbar.showSnackbar(errorText(context, e))
            }
        }
    }

    val all = review?.repos.orEmpty().flatMap { it.changes }
    ShedScaffold(
        title = stringResource(R.string.ai_review),
        subtitle = if (all.isNotEmpty()) stringResource(R.string.review_viewed, all.count { it.viewed }, all.size) else null,
        onBack = { nav.popBackStack() },
        actions = {
            if (canChange && all.isNotEmpty()) {
                TextButton(onClick = { explain(all.map { it.key }) }) {
                    Icon(Icons.Default.AutoAwesome, null)
                    HSpace(4)
                    Text(stringResource(R.string.review_explain_all))
                }
            }
            IconButton(onClick = { refresh++ }) { Icon(Icons.Default.Refresh, stringResource(R.string.refresh)) }
        },
    ) { padding ->
        val r = review
        when {
            r == null && error != null -> EmptyNote(error!!, Modifier.padding(padding))
            r == null -> Loading(Modifier.padding(padding))
            all.isEmpty() -> EmptyNote(stringResource(R.string.review_none), Modifier.padding(padding))
            else -> LazyColumn(Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(bottom = 32.dp)) {
                for (repo in r.repos) {
                    if (repo.changes.isEmpty()) continue
                    item(key = "repo-" + repo.root) { SectionTitle(tildePath(repo.root, home)) }
                    items(repo.changes, key = { it.key }) { change ->
                        Column {
                            Row(
                                Modifier.fillMaxWidth().clickable { open[change.key] = open[change.key] != true }.padding(start = 4.dp, end = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Checkbox(change.viewed, if (canChange) ({ setViewed(change, it) }) else null)
                                Text(change.status, fontFamily = Mono, color = statusColor(change.status), modifier = Modifier.width(18.dp))
                                Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                                    Text(change.path, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                    Text(layerName(change.layer), color = shed.textDim, style = MaterialTheme.typography.bodySmall)
                                }
                                if (canChange) {
                                    TextButton(enabled = explaining[change.key] != true, onClick = { explain(listOf(change.key)) }) {
                                        Text(stringResource(if (explaining[change.key] == true) R.string.review_explaining else R.string.review_explain))
                                    }
                                }
                            }
                            (explained[change.key] ?: change.explanation)?.let {
                                Column(
                                    Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
                                        .background(shed.accent.copy(alpha = 0.08f)).padding(10.dp),
                                ) {
                                    Text(stringResource(R.string.review_why), color = shed.accent, style = MaterialTheme.typography.labelSmall)
                                    MarkdownView(it)
                                }
                            }
                            if (open[change.key] == true) DiffView(change.diff)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun layerName(layer: String): String =
    if (layer == "uncommitted") stringResource(R.string.review_uncommitted) else stringResource(R.string.review_in_commit, layer)
