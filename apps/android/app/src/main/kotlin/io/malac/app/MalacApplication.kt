package io.malac.app

import android.app.Application
import io.malac.app.notify.BackgroundCheckWorker

class MalacApplication : Application() {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        // First, so a crash while starting up is kept too.
        val crashes = CrashLog(this).apply { install() }
        graph = AppGraph(this, crashes)
        graph.connections.start()
        graph.speech.start()
        BackgroundCheckWorker.schedule(this)
    }
}
