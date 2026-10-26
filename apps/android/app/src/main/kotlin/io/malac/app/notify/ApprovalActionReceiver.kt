package io.malac.app.notify

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import io.malac.app.MalacApplication
import io.malac.core.protocol.Decision
import kotlinx.coroutines.launch

/** Allow and Deny on an approval notification, answered without opening the app. */
class ApprovalActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val daemon = intent.getStringExtra(EXTRA_DAEMON) ?: return
        val request = intent.getStringExtra(EXTRA_REQUEST) ?: return
        val decision = intent.getStringExtra(EXTRA_DECISION)?.let(Decision::valueOf) ?: return
        val graph = (context.applicationContext as MalacApplication).graph
        val pending = goAsync()
        graph.scope.launch {
            try {
                graph.answer(daemon, request, decision)
            } finally {
                pending.finish()
            }
        }
    }

    companion object {
        const val EXTRA_DAEMON = "daemon"
        const val EXTRA_REQUEST = "request"
        const val EXTRA_DECISION = "decision"
    }
}
