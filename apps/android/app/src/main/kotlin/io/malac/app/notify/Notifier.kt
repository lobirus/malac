package io.malac.app.notify

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import io.malac.app.MainActivity
import io.malac.app.R
import io.malac.app.data.SavedDaemon
import io.malac.core.format.firstLine
import io.malac.core.format.truncate
import io.malac.core.protocol.Decision
import io.malac.core.protocol.PendingApproval
import io.malac.core.protocol.TurnStatus

/** Local notifications for approvals and finished turns, posted while the app is in the background. */
class Notifier(private val context: Context) {
    private val manager = NotificationManagerCompat.from(context)

    init {
        val system = context.getSystemService(NotificationManager::class.java)
        system.createNotificationChannel(
            NotificationChannel(APPROVALS, context.getString(R.string.channel_approvals), NotificationManager.IMPORTANCE_HIGH),
        )
        system.createNotificationChannel(
            NotificationChannel(TURNS, context.getString(R.string.channel_turns), NotificationManager.IMPORTANCE_DEFAULT),
        )
    }

    fun approval(daemon: SavedDaemon, approval: PendingApproval, place: String) {
        val id = approvalId(approval.id)
        val title = context.getString(R.string.notification_approval_title, approval.agentLabel.ifEmpty { "agent" }, approval.tool)
        val notification = NotificationCompat.Builder(context, APPROVALS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(truncate(firstLine(approval.summary), 200))
            .setSubText(listOf(place, daemon.name).filter { it.isNotEmpty() }.joinToString(" · "))
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setContentIntent(open(daemon, approval.agent, id))
            .addAction(0, context.getString(R.string.allow), decide(daemon, approval.id, Decision.Allow, id))
            .addAction(0, context.getString(R.string.deny), decide(daemon, approval.id, Decision.Deny, id))
            .setAutoCancel(true)
            .build()
        post(id, notification)
    }

    fun cancelApproval(requestId: String) = manager.cancel(approvalId(requestId))

    fun turnFinished(daemon: SavedDaemon, agent: String, name: String, status: TurnStatus, text: String) {
        val id = TURN_BASE + agent.hashCode()
        val title = when (status) {
            TurnStatus.Completed -> context.getString(R.string.notification_turn_done, name)
            TurnStatus.Failed -> context.getString(R.string.notification_turn_failed, name)
            TurnStatus.Cancelled -> context.getString(R.string.notification_turn_stopped, name)
        }
        val notification = NotificationCompat.Builder(context, TURNS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(truncate(firstLine(text), 200))
            .setStyle(NotificationCompat.BigTextStyle().bigText(truncate(text, 1_000)))
            .setSubText(daemon.name)
            .setContentIntent(open(daemon, agent, id))
            .setAutoCancel(true)
            .build()
        post(id, notification)
    }

    private fun post(id: Int, notification: android.app.Notification) {
        val granted = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        if (granted) manager.notify(id, notification)
    }

    private fun open(daemon: SavedDaemon, agent: String, id: Int): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            .putExtra(MainActivity.EXTRA_DAEMON, daemon.id)
            .putExtra(MainActivity.EXTRA_AGENT, agent)
        return PendingIntent.getActivity(context, id, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    private fun decide(daemon: SavedDaemon, request: String, decision: Decision, id: Int): PendingIntent {
        val intent = Intent(context, ApprovalActionReceiver::class.java)
            .putExtra(ApprovalActionReceiver.EXTRA_DAEMON, daemon.id)
            .putExtra(ApprovalActionReceiver.EXTRA_REQUEST, request)
            .putExtra(ApprovalActionReceiver.EXTRA_DECISION, decision.name)
        val code = id * 3 + decision.ordinal
        return PendingIntent.getBroadcast(context, code, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    private fun approvalId(requestId: String) = APPROVAL_BASE + requestId.hashCode()

    private companion object {
        const val APPROVALS = "approvals"
        const val TURNS = "turns"
        const val APPROVAL_BASE = 0x1000
        const val TURN_BASE = 0x2000
    }
}
