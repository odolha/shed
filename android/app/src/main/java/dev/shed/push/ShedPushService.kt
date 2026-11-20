package dev.shed.push

import android.util.Log
import dev.shed.shed
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

/** Where the UnifiedPush distributor delivers: endpoints and messages. */
class ShedPushService : PushService() {
    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        Push.newEndpoint(this, endpoint)
    }

    override fun onMessage(message: PushMessage, instance: String) {
        if (!message.decrypted) {
            Log.w("ShedPush", "a push that was not encrypted for this app; ignored")
            return
        }
        Push.show(this, message.content)
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        Log.w("ShedPush", "registration failed: $reason")
        if (reason == FailedReason.ACTION_REQUIRED || reason == FailedReason.VAPID_REQUIRED) {
            shed.settings.pushEnabled = false
        }
    }

    override fun onUnregistered(instance: String) {
        shed.settings.pushEnabled = false
        shed.settings.pushEndpoint = null
    }
}
