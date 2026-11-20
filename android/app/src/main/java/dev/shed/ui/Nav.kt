package dev.shed.ui

import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalContext
import androidx.navigation.NavHostController
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import dev.shed.Session
import dev.shed.api.Policy
import dev.shed.api.WorkspaceInfo
import dev.shed.api.WorkspaceState
import dev.shed.shed
import dev.shed.ui.ai.AiScreen
import dev.shed.ui.ai.ReviewScreen
import dev.shed.ui.bench.BenchScreen
import dev.shed.ui.bench.ClosedBenchScreen
import dev.shed.ui.bench.TabScreen
import dev.shed.ui.devices.DevicesScreen
import dev.shed.ui.git.GitScreen
import dev.shed.ui.logbook.LogbookScreen
import dev.shed.ui.settings.SettingsScreen
import dev.shed.ui.workspaces.WorkspacesScreen
import kotlinx.coroutines.flow.MutableStateFlow

/** The signed-in session, for every screen under `Nav`. */
val LocalSession = staticCompositionLocalOf<Session> { error("no session") }

/** Where the screens are, as routes. Arguments are URL-encoded. */
object Routes {
    const val WORKSPACES = "workspaces"
    const val DEVICES = "devices"
    const val SETTINGS = "settings"

    private fun e(text: String) = Uri.encode(text)

    fun bench(ws: String, bench: String) = "bench/${e(ws)}/${e(bench)}"
    fun closed(ws: String, name: String) = "closed/${e(ws)}/${e(name)}"
    fun tab(ws: String, tab: String) = "tab/${e(ws)}/${e(tab)}"
    fun ai(ws: String, tab: String) = "ai/${e(ws)}/${e(tab)}"
    fun review(ws: String, tab: String) = "review/${e(ws)}/${e(tab)}"
    fun git(ws: String, tab: String) = "git/${e(ws)}/${e(tab)}"
    fun logbook(ws: String, tab: String?) = "logbook/${e(ws)}?tab=${e(tab.orEmpty())}"
}

/** The workspace's last state, as it changes. */
@Composable
fun workspaceState(ws: String): WorkspaceState? {
    val states by LocalSession.current.socket.states.collectAsState()
    return states[ws]
}

@Composable
fun workspaceInfo(ws: String): WorkspaceInfo? {
    val list by LocalSession.current.socket.workspaces.collectAsState()
    return list.firstOrNull { it.id == ws }
}

/** Whether the workspace lets this phone change things (and is there to ask). */
@Composable
fun canChange(ws: String): Boolean {
    val info = workspaceInfo(ws) ?: return false
    return info.online && info.policy == Policy.Full
}

@Composable
fun Nav(link: MutableStateFlow<Uri?>) {
    val session by LocalContext.current.shed.session.collectAsState()
    val current = session ?: return
    val nav = rememberNavController()
    val pending by link.collectAsState()
    LaunchedEffect(pending) {
        val uri = pending ?: return@LaunchedEffect
        link.value = null
        open(nav, uri)
    }
    CompositionLocalProvider(LocalSession provides current) {
        NavHost(nav, startDestination = Routes.WORKSPACES) {
            composable(Routes.WORKSPACES) { WorkspacesScreen(nav) }
            composable(Routes.DEVICES) { DevicesScreen(nav) }
            composable(Routes.SETTINGS) { SettingsScreen(nav) }
            composable("bench/{ws}/{bench}") {
                BenchScreen(nav, it.arg("ws"), it.arg("bench"))
            }
            composable("closed/{ws}/{name}") {
                ClosedBenchScreen(nav, it.arg("ws"), it.arg("name"))
            }
            composable("tab/{ws}/{tab}") { TabScreen(nav, it.arg("ws"), it.arg("tab")) }
            composable("ai/{ws}/{tab}") { AiScreen(nav, it.arg("ws"), it.arg("tab")) }
            composable("review/{ws}/{tab}") { ReviewScreen(nav, it.arg("ws"), it.arg("tab")) }
            composable("git/{ws}/{tab}") { GitScreen(nav, it.arg("ws"), it.arg("tab")) }
            composable(
                "logbook/{ws}?tab={tab}",
                arguments = listOf(navArgument("tab") { type = NavType.StringType; defaultValue = "" }),
            ) {
                LogbookScreen(nav, it.arg("ws"), it.arg("tab").ifEmpty { null })
            }
        }
    }
}

private fun androidx.navigation.NavBackStackEntry.arg(name: String): String =
    arguments?.getString(name).orEmpty()

/** Opens a `shed://w/{workspace}/b/{bench}/t/{tab}` link (bench and tab optional). */
fun open(nav: NavHostController, uri: Uri) {
    val target = parseLink(uri) ?: return
    nav.popBackStack(Routes.WORKSPACES, inclusive = false)
    when {
        target.tab != null -> {
            if (target.bench != null) nav.navigate(Routes.bench(target.workspace, target.bench))
            nav.navigate(Routes.tab(target.workspace, target.tab))
        }
        target.bench != null -> nav.navigate(Routes.bench(target.workspace, target.bench))
    }
}

data class LinkTarget(val workspace: String, val bench: String?, val tab: String?)

/** The parts of a `shed://w/…` link, or `null` if it is not one. */
fun parseLink(uri: Uri): LinkTarget? {
    if (uri.scheme != "shed" || uri.host != "w") return null
    val parts = uri.pathSegments
    val workspace = parts.getOrNull(0) ?: return null
    val bench = if (parts.getOrNull(1) == "b") parts.getOrNull(2) else null
    val tab = if (parts.getOrNull(3) == "t") parts.getOrNull(4) else null
    return LinkTarget(workspace, bench, tab)
}
