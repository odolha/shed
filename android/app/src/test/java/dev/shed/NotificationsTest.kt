package dev.shed

import dev.shed.api.Notification
import dev.shed.api.NotificationFrame
import dev.shed.api.NotificationKind
import dev.shed.api.Policy
import dev.shed.api.PushInfo
import dev.shed.api.PushPrefs
import dev.shed.api.WorkspaceInfo
import dev.shed.net.ShedJson
import dev.shed.push.PushMode
import dev.shed.push.leaveUnifiedPush
import dev.shed.push.pushMode
import dev.shed.ui.liveNoticeText
import dev.shed.ui.showsTab
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Where notifications come from: a ntfy topic or UnifiedPush, and live ones. */
class NotificationsTest {
    private val prefs = PushPrefs(listOf(NotificationKind.AiWaiting))

    @Test
    fun aNtfyTopicReplacesUnifiedPush() {
        val info = ShedJson.decodeFromString(
            PushInfo.serializer(),
            """{"vapidKey":"BKey","prefs":{"kinds":["aiWaiting"]},"subscribed":true,"ntfyTopic":"https://ntfy.example.com/alerts-shed"}""",
        )
        val mode = pushMode(info)
        assertEquals(PushMode.Ntfy("https://ntfy.example.com/alerts-shed"), mode)
        // Registered for UnifiedPush from before: leave it; not registered: nothing to do.
        assertTrue(leaveUnifiedPush(mode, registered = true))
        assertFalse(leaveUnifiedPush(mode, registered = false))
    }

    @Test
    fun withoutATopicUnifiedPushAsBefore() {
        // An older nexus says nothing of a topic.
        val old = ShedJson.decodeFromString(PushInfo.serializer(), """{"vapidKey":"BKey","prefs":{"kinds":[]}}""")
        assertEquals(PushMode.Unified("BKey"), pushMode(old))
        assertFalse(leaveUnifiedPush(pushMode(old), registered = true))
        assertEquals(PushMode.Unified("BKey"), pushMode(PushInfo("BKey", prefs, true, " ")))
        assertEquals(PushMode.Off, pushMode(PushInfo(null, prefs, false, null)))
    }

    @Test
    fun liveNotificationsSayWhereAndSkipTheTabOpen() {
        val frame = NotificationFrame(
            "w1",
            Notification(NotificationKind.AiWaiting, "Claude waits for an answer", "api · claude", "b1", "t1"),
        )
        val desk = WorkspaceInfo("w1", "desk", true, 1u, Policy.Full, "0.1.0")
        assertEquals("Claude waits for an answer\ndesk · api · claude", liveNoticeText(frame, listOf(desk)))
        val offline = NotificationFrame("w1", Notification(NotificationKind.WorkspaceOffline, "desk went offline"))
        assertEquals("desk went offline\ndesk", liveNoticeText(offline, listOf(desk)))
        assertEquals("desk went offline", liveNoticeText(offline, emptyList()))

        assertTrue(showsTab("ai/{ws}/{tab}", "w1", "t1", "w1", "t1"))
        assertTrue(showsTab("tab/{ws}/{tab}", "w1", "t1", "w1", "t1"))
        assertFalse(showsTab("git/{ws}/{tab}", "w1", "t1", "w1", "t1"))
        assertFalse(showsTab("ai/{ws}/{tab}", "w2", "t1", "w1", "t1"))
        assertFalse(showsTab("workspaces", "", "", "w1", null))
    }
}
