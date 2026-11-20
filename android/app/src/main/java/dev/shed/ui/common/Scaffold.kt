package dev.shed.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.shed.R
import dev.shed.net.Connection
import dev.shed.shed
import dev.shed.ui.theme.LocalShed

/** A screen: a top bar (title, back, actions), the connection's state under it, and the snackbar. */
@Composable
fun ShedScaffold(
    title: String,
    onBack: (() -> Unit)?,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    accent: Color? = null,
    actions: @Composable RowScope.() -> Unit = {},
    bottomBar: @Composable () -> Unit = {},
    floatingActionButton: @Composable () -> Unit = {},
    content: @Composable (PaddingValues) -> Unit,
) {
    val shed = LocalShed.current
    Scaffold(
        modifier = modifier,
        topBar = {
            Column {
                TopAppBar(
                    title = {
                        Column {
                            Text(
                                title,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                color = accent?.let { shed.ink(it) } ?: shed.text,
                                style = MaterialTheme.typography.titleMedium,
                            )
                            if (!subtitle.isNullOrEmpty()) {
                                Text(
                                    subtitle,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    color = shed.textDim,
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                        }
                    },
                    navigationIcon = {
                        if (onBack != null) {
                            IconButton(onClick = onBack) {
                                Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                            }
                        }
                    },
                    actions = actions,
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = shed.surface),
                )
                ConnectionStrip()
            }
        },
        bottomBar = bottomBar,
        floatingActionButton = floatingActionButton,
        snackbarHost = { SnackbarHost(LocalSnackbar.current) },
        containerColor = shed.background,
        content = content,
    )
}

/** A thin line under the top bar while the nexus is not reached. */
@Composable
fun ConnectionStrip() {
    val shed = LocalShed.current
    val session by LocalContext.current.shed.session.collectAsState()
    val socket = session?.socket ?: return
    val connection by socket.connection.collectAsState()
    when (val c = connection) {
        is Connection.Connecting -> LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp))
        is Connection.Offline -> Text(
            stringResource(R.string.connection_offline, c.reason),
            color = shed.background,
            style = MaterialTheme.typography.labelSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier
                .fillMaxWidth()
                .background(shed.warn)
                .padding(horizontal = 16.dp, vertical = 2.dp),
        )
        is Connection.Closed -> Text(
            stringResource(R.string.connection_closed, c.error.message.orEmpty()),
            color = shed.background,
            style = MaterialTheme.typography.labelSmall,
            modifier = Modifier
                .fillMaxWidth()
                .background(shed.danger)
                .padding(horizontal = 16.dp, vertical = 2.dp),
        )
        else -> {}
    }
}
