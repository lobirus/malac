package io.malac.app.data

import android.content.Context
import androidx.core.content.edit
import io.malac.core.state.Ledger

/** What background checks already notified about, per daemon. */
class LedgerStore(context: Context) {
    private val prefs = context.getSharedPreferences("ledgers", Context.MODE_PRIVATE)

    fun get(daemon: String) = Ledger(
        requests = prefs.getStringSet("$daemon.requests", null).orEmpty(),
        running = prefs.getStringSet("$daemon.running", null).orEmpty(),
    )

    fun put(daemon: String, ledger: Ledger) = prefs.edit {
        putStringSet("$daemon.requests", ledger.requests)
        putStringSet("$daemon.running", ledger.running)
    }

    fun remove(daemon: String) = prefs.edit {
        remove("$daemon.requests")
        remove("$daemon.running")
    }
}
