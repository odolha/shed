package dev.shed.ui

import dev.shed.api.NotificationFrame
import dev.shed.api.WorkspaceInfo

/** A live notification's text: its title, then the workspace and where in it. */
fun liveNoticeText(frame: NotificationFrame, workspaces: List<WorkspaceInfo>): String {
    val n = frame.notification
    val name = workspaces.firstOrNull { it.id == frame.workspace }?.name
    val where = listOfNotNull(name, n.body).filter { it.isNotBlank() }.joinToString(" · ")
    return if (where.isEmpty()) n.title else n.title + "\n" + where
}

/** Whether the screen at [route] (with its `ws` and `tab`) shows that workspace's [tab]. */
fun showsTab(route: String?, ws: String?, shown: String?, workspace: String, tab: String?): Boolean =
    tab != null && ws == workspace && shown == tab && route in listOf("tab/{ws}/{tab}", "ai/{ws}/{tab}")
