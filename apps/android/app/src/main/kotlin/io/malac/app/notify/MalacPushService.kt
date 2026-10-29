package io.malac.app.notify

import android.util.Log
import io.malac.app.MalacApplication
import io.malac.core.state.parsePush
import kotlinx.coroutines.launch
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

/**
 * What the UnifiedPush distributor hands malac: endpoints to register with
 * a daemon and the daemon's pushes, one registration ([instance]) per
 * saved daemon, named by its id. The push library has decrypted messages
 * by the time they arrive here.
 */
class MalacPushService : PushService() {
    private val graph get() = (application as MalacApplication).graph

    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        graph.scope.launch { graph.push.newEndpoint(instance, endpoint) }
    }

    override fun onMessage(message: PushMessage, instance: String) {
        val push = if (message.decrypted) parsePush(message.content) else null
        graph.scope.launch { graph.push.received(instance, push) }
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        Log.w("malac-push", "registration for $instance failed: $reason")
        graph.push.refresh()
    }

    override fun onUnregistered(instance: String) {
        graph.scope.launch { graph.push.unregistered(instance) }
    }
}
