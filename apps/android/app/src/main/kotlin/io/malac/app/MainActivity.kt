package io.malac.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import io.malac.app.ui.MalacApp
import io.malac.app.ui.OpenAgent
import io.malac.app.ui.theme.MalacTheme
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow

class MainActivity : ComponentActivity() {
    private val opens = Channel<OpenAgent>(Channel.CONFLATED)
    private val openRequests = opens.receiveAsFlow()

    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) handle(intent)
        val asked = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        if (asked) notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        val graph = (application as MalacApplication).graph
        setContent {
            MalacTheme {
                MalacApp(graph, openRequests)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent) {
        val daemon = intent.getStringExtra(EXTRA_DAEMON) ?: return
        val agent = intent.getStringExtra(EXTRA_AGENT) ?: return
        opens.trySend(OpenAgent(daemon, agent))
    }

    companion object {
        const val EXTRA_DAEMON = "daemon"
        const val EXTRA_AGENT = "agent"
    }
}
