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
import dev.shed.api.PushSubscribe
import dev.shed.net.Empty
import dev.shed.net.ShedJson
import dev.shed.shed
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint

/** What the nexus pushes, once decrypted (`shed-nexus/src/push.rs`, `Payload`). */
@Serializable
data class PushPayload(
    val kind: NotificationKind,
    val title: String,
    val body: String = "",
    val workspace: String,
    val workspaceName: String = "",
    val bench: String? = null,
    val tab: String? = null,
)

/** A place in Shed, as a link: `shed://w/{workspace}/b/{bench}/t/{tab}`. */
fun shedLink(workspace: String, bench: String? = null, tab: String? = null): Uri {
    var text = "shed://w/" + Uri.encode(workspace)
    if (bench != null) {
        text += "/b/" + Uri.encode(bench)
        if (tab != null) text += "/t/" + Uri.encode(tab)
    }
    return text.toUri()
}

/**
 * Push through UnifiedPush: a distributor app (ntfy) on the phone holds the
 * connection to a push server; the nexus sends to the endpoint it gives,
 * encrypted for this app (RFC 8291; the connector library keeps the keys and
 * decrypts). No Google services.
 */
object Push {
    private const val TAG = "ShedPush"

    fun channelId(kind: NotificationKind) = "shed-" + kind.string

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val channels = listOf(
            NotificationKind.AiWaiting to R.string.channel_ai_waiting,
            NotificationKind.AiDone to R.string.channel_ai_done,
            NotificationKind.CommandDone to R.string.channel_command_done,
            NotificationKind.WorkspaceOffline to R.string.channel_workspace_offline,
        )
        for ((kind, name) in channels) {
            val importance = if (kind == NotificationKind.AiWaiting) {
                NotificationManager.IMPORTANCE_HIGH
            } else {
                NotificationManager.IMPORTANCE_DEFAULT
            }
            manager.createNotificationChannel(NotificationChannel(channelId(kind), context.getString(name), importance))
        }
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

    /** Registers again (a new sign-in, an app start), if push is on. */
    fun refresh(context: Context) {
        val app = context.shed
        if (!app.settings.pushEnabled || UnifiedPush.getAckDistributor(context) == null) return
        val session = app.session.value ?: return
        app.scope.launch {
            runCatching {
                val info = session.http.get("/push", PushInfo.serializer())
                info.vapidKey?.let { UnifiedPush.register(context, vapid = it) }
            }.onFailure { Log.d(TAG, "push refresh: ${it.message}") }
        }
    }

    fun unregister(context: Context) {
        runCatching { UnifiedPush.unregister(context) }
        context.shed.settings.pushEnabled = false
        context.shed.settings.pushEndpoint = null
    }

    /** The distributor gave an endpoint: hands it to the nexus. */
    fun newEndpoint(context: Context, endpoint: PushEndpoint) {
        val app = context.shed
        val keys = endpoint.pubKeySet ?: run {
            Log.w(TAG, "an endpoint without keys; the nexus needs Web Push keys")
            return
        }
        app.settings.pushEndpoint = endpoint.url
        val session = app.session.value ?: return
        app.scope.launch {
            runCatching {
                session.http.post(
                    "/push/subscribe",
                    PushSubscribe(endpoint.url, keys.pubKey, keys.auth),
                    PushSubscribe.serializer(),
                    Empty.serializer(),
                )
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
        val intent = Intent(Intent.ACTION_VIEW, shedLink(payload.workspace, payload.bench, payload.tab), context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        val tap = PendingIntent.getActivity(
            context,
            (payload.workspace + payload.bench + payload.tab).hashCode(),
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = listOf(payload.body, payload.workspaceName).filter { it.isNotBlank() }.joinToString(" · ")
        val notification = NotificationCompat.Builder(context, channelId(payload.kind))
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(payload.title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setCategory(if (payload.kind == NotificationKind.AiWaiting) NotificationCompat.CATEGORY_REMINDER else NotificationCompat.CATEGORY_STATUS)
            .build()
        val id = (payload.kind.string + payload.workspace + payload.tab).hashCode()
        NotificationManagerCompat.from(context).notify(id, notification)
    }
}
