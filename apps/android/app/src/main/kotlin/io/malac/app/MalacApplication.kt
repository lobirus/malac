package io.malac.app

import android.app.Application
import io.malac.app.notify.BackgroundCheckWorker

class MalacApplication : Application() {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        graph = AppGraph(this)
        graph.connections.start()
        BackgroundCheckWorker.schedule(this)
    }
}
