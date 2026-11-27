package dev.shed.push

import android.Manifest
import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.core.net.toUri
import dev.shed.MainActivity
import dev.shed.R
import dev.shed.api.NotificationKind
import dev.shed.api.PushInfo
import dev.shed.api.PushPayload
import dev.shed.api.PushSubscribe
import dev.shed.net.Empty
import dev.shed.net.ShedJson
import dev.shed.shed
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint

/** A place in Shed, as a link: `shed://w/{workspace}/b/{bench}/t/{tab}`. */
fun shedLink(workspace: String, bench: String? = null, tab: String? = null): Uri {
    var text = "shed://w/" + Uri.encode(workspace)
    if (bench != null) {
        text += "/b/" + Uri.encode(bench)
        if (tab != null) text += "/t/" + Uri.encode(tab)
    }
    return text.toUri()
}

/** The Devices screen, where a security alert leads (a device can be revoked there). */
val DEVICES_LINK: Uri = "shed://devices".toUri()

/** How this phone hears about things while no Shed app is in view. */
sealed interface PushMode {
    /** The nexus publishes to a ntfy topic: subscribe to it in the ntfy app. No UnifiedPush then. */
    data class Ntfy(val topic: String) : PushMode

    /** UnifiedPush, registered with the nexus's VAPID key. */
    data class Unified(val vapid: String) : PushMode

    /** The nexus pushes nothing. */
    data object Off : PushMode
}

/** What the nexus's `PushInfo` means for this phone. */
fun pushMode(info: PushInfo): PushMode {
    val topic = info.ntfyTopic?.trim().orEmpty()
    val vapid = info.vapidKey
    return when {
        topic.isNotEmpty() -> PushMode.Ntfy(topic)
        vapid != null -> PushMode.Unified(vapid)
        else -> PushMode.Off
    }
}

/** Whether a UnifiedPush registration here should go: the nexus reaches phones through its topic. */
fun leaveUnifiedPush(mode: PushMode, registered: Boolean): Boolean = mode is PushMode.Ntfy && registered

/**
 * Push when no Shed app is in view. With a ntfy topic at the nexus
 * ([PushMode.Ntfy]) the phone reads it in the ntfy app and this app does not
 * register. Otherwise UnifiedPush: a distributor app (ntfy) on the phone
 * holds the connection to a push server; the nexus sends to the endpoint it
 * gives, encrypted for this app (RFC 8291; the connector library keeps the
 * keys and decrypts). No Google services.
 */
object Push {
    private const val TAG = "ShedPush"

    /** Bumped when this phone's subscription at the nexus changed, for Settings to look again. */
    val changed = MutableStateFlow(0)

    /** A channel's importance is fixed once made, so a new level needs a new id. */
    fun channelId(kind: NotificationKind) = when (kind) {
        NotificationKind.WorkspaceOffline -> "shed-workspaceOffline-low"
        NotificationKind.Security -> "shed-security-high"
        else -> "shed-" + kind.string
    }

    /** Channels made by earlier versions, at levels since changed. */
    private val oldChannels = listOf("shed-workspaceOffline")

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val channels = listOf(
            NotificationKind.AiWaiting to R.string.channel_ai_waiting,
            NotificationKind.AiDone to R.string.channel_ai_done,
            NotificationKind.CommandDone to R.string.channel_command_done,
            NotificationKind.WorkspaceOffline to R.string.channel_workspace_offline,
            NotificationKind.Security to R.string.channel_security,
        )
        for ((kind, name) in channels) {
            val importance = when (kind) {
                NotificationKind.AiWaiting, NotificationKind.Security -> NotificationManager.IMPORTANCE_HIGH
                NotificationKind.WorkspaceOffline -> NotificationManager.IMPORTANCE_LOW
                else -> NotificationManager.IMPORTANCE_DEFAULT
            }
            manager.createNotificationChannel(NotificationChannel(channelId(kind), context.getString(name), importance))
        }
        oldChannels.forEach(manager::deleteNotificationChannel)
    }

    /** Distributors installed on the phone (ntfy and the like). */
    fun distributors(context: Context): List<String> = UnifiedPush.getDistributors(context)

    /** Picks a distributor (asking if there are several) and registers. */
    fun enable(activity: Activity, vapid: String, done: (Boolean) -> Unit) {
        UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { ok ->
            if (ok) {
                activity.shed.settings.pushEnabled = true
                runCatching { UnifiedPush.register(activity, vapid = vapid) }
                    .onFailure { Log.w(TAG, "register failed", it) }
            }
            done(ok)
        }
    }

    /** Whether this phone is registered for UnifiedPush (or thinks so). */
    fun registered(context: Context): Boolean =
        context.shed.settings.pushEnabled || context.shed.settings.pushEndpoint != null

    /**
     * Registers again (a new sign-in, an app start), if push is on; leaves
     * UnifiedPush when the nexus has a ntfy topic for phones instead.
     */
    fun refresh(context: Context) {
        val app = context.shed
        if (!registered(context)) return
        val session = app.session.value ?: return
        app.scope.launch {
            runCatching {
                val mode = pushMode(session.http.get("/push", PushInfo.serializer()))
                when {
                    leaveUnifiedPush(mode, registered(context)) -> disable(context)
                    mode is PushMode.Unified && app.settings.pushEnabled && UnifiedPush.getAckDistributor(context) != null ->
                        UnifiedPush.register(context, vapid = mode.vapid)
                }
            }.onFailure { Log.d(TAG, "push refresh: ${it.message}") }
        }
    }

    /** Unregisters from the distributor, here only (signing out: the nexus drops the device). */
    fun unregister(context: Context) {
        runCatching { UnifiedPush.unregister(context) }
        context.shed.settings.pushEnabled = false
        context.shed.settings.pushEndpoint = null
        context.shed.settings.pendingEndpoint = null
    }

    /**
     * Turns push off on this phone: tells the nexus to stop pushing here
     * (`push/unsubscribe`; [done] with whether it did) and unregisters from
     * the distributor.
     */
    fun disable(context: Context, done: (Boolean) -> Unit = {}) {
        val app = context.shed
        unregister(context)
        val session = app.session.value ?: return done(false)
        app.scope.launch {
            val ok = runCatching { session.http.post("/push/unsubscribe", Empty.serializer()) }
                .onFailure { Log.w(TAG, "unsubscribe failed: ${it.message}") }
                .isSuccess
            changed.value++
            done(ok)
        }
    }

    /**
     * The distributor gave an endpoint: hands it to the nexus. While the app
     * is locked (or not yet unlocked since it started) its token cannot be
     * read: the endpoint waits, and goes once unlocked ([sendPendingEndpoint]).
     */
    fun newEndpoint(context: Context, endpoint: PushEndpoint) {
        val app = context.shed
        val keys = endpoint.pubKeySet ?: run {
            Log.w(TAG, "an endpoint without keys; the nexus needs Web Push keys")
            return
        }
        app.settings.pushEndpoint = endpoint.url
        app.settings.pendingEndpoint = listOf(endpoint.url, keys.pubKey, keys.auth).joinToString(" ")
        sendPendingEndpoint(context)
    }

    /** Sends the endpoint that waits, if any, once there is a session (unlocked). */
    fun sendPendingEndpoint(context: Context) {
        val app = context.shed
        val parts = app.settings.pendingEndpoint?.split(" ")?.takeIf { it.size == 3 } ?: return
        val session = app.session.value ?: return
        app.scope.launch {
            runCatching {
                session.http.post(
                    "/push/subscribe",
                    PushSubscribe(parts[0], parts[1], parts[2]),
                    PushSubscribe.serializer(),
                    Empty.serializer(),
                )
                app.settings.pendingEndpoint = null
                changed.value++
            }.onFailure { Log.w(TAG, "subscribe failed: ${it.message}") }
        }
    }

    /** A push came: shows it. */
    fun show(context: Context, content: ByteArray) {
        val payload = runCatching {
            ShedJson.decodeFromString(PushPayload.serializer(), content.decodeToString())
        }.getOrElse {
            Log.w(TAG, "a push that does not read: ${it.message}")
            return
        }
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val security = payload.kind == NotificationKind.Security
        val link = if (security) DEVICES_LINK else shedLink(payload.workspace, payload.bench, payload.tab)
        val intent = Intent(Intent.ACTION_VIEW, link, context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        val tap = PendingIntent.getActivity(
            context,
            if (security) payload.title.hashCode() else (payload.workspace + payload.bench + payload.tab).hashCode(),
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = listOfNotNull(payload.body, payload.workspaceName).filter { it.isNotBlank() }.joinToString(" · ")
        val notification = NotificationCompat.Builder(context, channelId(payload.kind))
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(payload.title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setCategory(
                when (payload.kind) {
                    NotificationKind.AiWaiting -> NotificationCompat.CATEGORY_REMINDER
                    NotificationKind.Security -> NotificationCompat.CATEGORY_ERROR
                    else -> NotificationCompat.CATEGORY_STATUS
                },
            )
            .setPriority(if (security) NotificationCompat.PRIORITY_HIGH else NotificationCompat.PRIORITY_DEFAULT)
            .build()
        // Each security alert stays (another does not replace it).
        val id = if (security) (payload.title + payload.body).hashCode() else (payload.kind.string + payload.workspace + payload.tab).hashCode()
        NotificationManagerCompat.from(context).notify(id, notification)
    }
}
