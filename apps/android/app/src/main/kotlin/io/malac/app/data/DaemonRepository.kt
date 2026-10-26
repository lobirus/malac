package io.malac.app.data

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import io.malac.core.net.Endpoint
import io.malac.core.protocol.MalacJson
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import java.util.UUID

/** A daemon this phone paired with. Its token is in the [TokenVault], not here. */
@Serializable
data class SavedDaemon(
    val id: String,
    val name: String,
    /** The daemon's base address, e.g. `http://192.168.1.20:7700/`. */
    val url: String,
    /** The id the daemon gave this device, for `malac remote devices`. */
    val deviceId: String? = null,
    val added: Long = System.currentTimeMillis(),
)

private val Context.store by preferencesDataStore("daemons")

/** Saved daemons and the selected one. */
class DaemonRepository(private val context: Context, private val vault: TokenVault) {
    private val listKey = stringPreferencesKey("daemons")
    private val selectedKey = stringPreferencesKey("selected")
    private val serializer = ListSerializer(SavedDaemon.serializer())

    val daemons: Flow<List<SavedDaemon>> = context.store.data.map { prefs ->
        prefs[listKey]?.let { MalacJson.decodeFromString(serializer, it) }.orEmpty()
    }

    /** The selected daemon, or the first one. */
    val selected: Flow<SavedDaemon?> = context.store.data.map { prefs ->
        val list = prefs[listKey]?.let { MalacJson.decodeFromString(serializer, it) }.orEmpty()
        list.firstOrNull { it.id == prefs[selectedKey] } ?: list.firstOrNull()
    }

    suspend fun add(name: String, base: HttpUrl, token: String, deviceId: String?): SavedDaemon {
        val daemon = SavedDaemon(UUID.randomUUID().toString(), name, base.toString(), deviceId)
        vault.put(daemon.id, token)
        edit { it + daemon }
        select(daemon.id)
        return daemon
    }

    suspend fun rename(id: String, name: String) = edit { list -> list.map { if (it.id == id) it.copy(name = name) else it } }

    suspend fun forget(id: String) {
        edit { list -> list.filterNot { it.id == id } }
        vault.remove(id)
    }

    suspend fun select(id: String) {
        context.store.edit { it[selectedKey] = id }
    }

    fun endpoint(daemon: SavedDaemon): Endpoint? {
        val base = daemon.url.toHttpUrlOrNull() ?: return null
        val token = vault.get(daemon.id) ?: return null
        return Endpoint(base, token)
    }

    private suspend fun edit(change: (List<SavedDaemon>) -> List<SavedDaemon>) {
        context.store.edit { prefs ->
            val list = prefs[listKey]?.let { MalacJson.decodeFromString(serializer, it) }.orEmpty()
            prefs[listKey] = MalacJson.encodeToString(serializer, change(list))
        }
    }
}
