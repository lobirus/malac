package io.malac.core.net

import io.malac.core.pairing.Paired
import io.malac.core.pairing.PairingService
import io.malac.core.protocol.Command
import io.malac.core.protocol.DeviceList
import io.malac.core.protocol.DeviceView
import io.malac.core.protocol.Hello
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.PushStatus
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.SessionList
import io.malac.core.protocol.SessionSummary
import io.malac.core.protocol.SessionTranscript
import io.malac.core.protocol.Transcription
import io.malac.core.protocol.VoiceStatus
import io.malac.core.protocol.encode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationException
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.File
import java.io.IOException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Where a daemon is and the token this device uses for it. */
data class Endpoint(val base: HttpUrl, val token: String) {
    override fun toString() = "Endpoint($base)"
}

/** A failed request; [status] is the HTTP status, 0 when the daemon could not be reached. */
class DaemonException(val status: Int, message: String) : IOException(message) {
    /** The token was revoked or never valid: pair again. */
    val unauthorized: Boolean get() = status == 401

    /** Refused: a wrong, used or expired pairing code, or remote access is off. */
    val forbidden: Boolean get() = status == 403

    /** Too many wrong pairing codes; a new code is needed. */
    val tooManyCodes: Boolean get() = status == 429
}

private val JSON = "application/json; charset=utf-8".toMediaType()
private val WAV = "audio/wav".toMediaType()

/** A file or clip from the daemon, with the type it says it is. */
class Download(val bytes: ByteArray, val mediaType: String)

/** The daemon's REST API, for what the WebSocket does not carry and for background checks. */
class DaemonApi(private val client: OkHttpClient, private val endpoint: Endpoint) {
    suspend fun session(id: String): SessionTranscript = get("api/sessions/$id", SessionTranscript.serializer())

    suspend fun sessions(limit: Int = 50): List<SessionSummary> =
        get("api/sessions?limit=$limit", SessionList.serializer()).sessions

    /** The same snapshot the WebSocket starts with. */
    suspend fun snapshot(): ServerMessage.Snapshot = get("api/snapshot", ServerMessage.Snapshot.serializer())

    /** This machine's paired devices; the one making the request has `current`. */
    suspend fun devices(): List<DeviceView> = get("api/devices", DeviceList.serializer()).devices

    /** Revokes a device: its token stops working and its connections close. */
    suspend fun revoke(device: String): DeviceView {
        val request = authorized("api/devices/$device").delete().build()
        return client.fetch(request, Revoked.serializer()).revoked
    }

    /** Unpairs this device, when the daemon answers and knows it. */
    suspend fun revokeSelf() {
        devices().firstOrNull { it.current }?.let { revoke(it.id) }
    }

    @Serializable
    private data class Revoked(val revoked: DeviceView)

    /** Runs a command without a WebSocket, e.g. from a notification action. */
    suspend fun command(command: Command): ServerMessage {
        val request = authorized("api/command").post(command.encode().toRequestBody(JSON)).build()
        return client.fetch(request, ServerMessage.serializer())
    }

    /** The daemon's VAPID key, which a push distributor needs, and whether this device gets pushes. */
    suspend fun push(): PushStatus = get("api/push", PushStatus.serializer())

    /** Registers the Web Push endpoint a distributor gave this device, and the keys that encrypt to it. */
    suspend fun registerPush(endpoint: String, p256dh: String, auth: String) {
        val body = MalacJson.encodeToString(PushRegistration.serializer(), PushRegistration(endpoint, p256dh, auth))
        client.fetch(authorized("api/push").put(body.toRequestBody(JSON)).build(), JsonObject.serializer())
    }

    suspend fun unregisterPush() {
        client.fetch(authorized("api/push").delete().build(), JsonObject.serializer())
    }

    @Serializable
    private data class PushRegistration(val endpoint: String, val p256dh: String, val auth: String)

    /** What dictation, speech and talk mode offer; [refresh] reads the providers' model lists again first. */
    suspend fun voice(refresh: Boolean = false): VoiceStatus =
        get(if (refresh) "api/voice?refresh=true" else "api/voice", VoiceStatus.serializer())

    /**
     * Changes voice settings: any of `dictation`, `dictation_model`,
     * `language`, `speech`, `speech_model`, `speech_voice`, `talk`,
     * `talk_model` and `talk_voice`; JSON null removes one.
     */
    suspend fun setVoice(changes: Map<String, JsonElement>): VoiceStatus {
        val request = authorized("api/voice").put(JsonObject(changes).toString().toRequestBody(JSON)).build()
        return client.fetch(request, VoiceStatus.serializer())
    }

    /** Dictation: [wav] is a 16 kHz mono 16-bit recording. */
    suspend fun transcribe(wav: ByteArray, language: String? = null): Transcription {
        val url = endpoint.base.resolve("api/voice/transcribe")!!.newBuilder()
            .apply { if (!language.isNullOrEmpty()) addQueryParameter("language", language) }
            .build()
        val request = authorized(url).post(wav.toRequestBody(WAV)).build()
        return client.fetch(request, Transcription.serializer())
    }

    /** The audio of a `say` broadcast: `audio/mpeg` or `audio/wav`. */
    suspend fun sayAudio(id: String): Download = client.download(authorized(url("api/voice/say", id)).get().build())

    /** A file an agent sent with `send_to_user`, read into memory, as images are. */
    suspend fun shared(id: String): Download = client.download(authorized(url("api/shared", id)).get().build())

    /** Saves a shared file to [file] as it arrives, for files too big to hold in memory. */
    suspend fun saveShared(id: String, file: File) {
        val url = url("api/shared", id).newBuilder().addQueryParameter("download", "true").build()
        val response = client.newCall(authorized(url).get().build()).await()
        val partial = File(file.path + ".part")
        response.use {
            withContext(Dispatchers.IO) {
                try {
                    it.requireSuccess()
                    file.parentFile?.mkdirs()
                    partial.outputStream().use { out -> it.body.byteStream().copyTo(out) }
                    if (!partial.renameTo(file)) throw IOException("could not save ${file.name}")
                } catch (error: IOException) {
                    partial.delete()
                    throw error as? DaemonException ?: DaemonException(0, error.message ?: "the connection dropped")
                }
            }
        }
    }

    private suspend fun <T> get(path: String, deserializer: DeserializationStrategy<T>): T {
        val request = authorized(path).get().build()
        return client.fetch(request, deserializer)
    }

    private fun authorized(path: String): Request.Builder = authorized(endpoint.base.resolve(path)!!)

    private fun authorized(url: HttpUrl): Request.Builder =
        Request.Builder().url(url).header("Authorization", "Bearer ${endpoint.token}")

    /** [path] with [id] as its last segment, encoded. */
    private fun url(path: String, id: String): HttpUrl = endpoint.base.resolve(path)!!.newBuilder().addPathSegment(id).build()
}

/**
 * Pairing as `docs/remote.md` describes it for apps: `GET /api/hello`
 * confirms the address is a malac daemon, then `POST /api/pair` trades the
 * one-time code for a device token. Neither needs a token.
 */
class HttpPairingService(private val client: OkHttpClient) : PairingService {
    @Serializable
    private data class PairRequest(val code: String, val name: String, val kind: String)

    @Serializable
    private data class PairResponse(val token: String, val device: DeviceView? = null, val version: String = "")

    suspend fun hello(base: HttpUrl): Hello {
        val request = Request.Builder().url(base.resolve("api/hello")!!).get().build()
        return client.fetch(request, Hello.serializer())
    }

    override suspend fun pair(base: HttpUrl, code: String, deviceName: String): Paired {
        // Unreachable, or refused with a reason (remote access off): say so. Anything else is not malac.
        val hello = try {
            hello(base)
        } catch (error: DaemonException) {
            if (error.status == 0 || error.forbidden) throw error
            null
        }
        if (hello?.app != "malac") throw DaemonException(0, "$base is not a malac daemon")
        val body = MalacJson.encodeToString(PairRequest.serializer(), PairRequest(code, deviceName, kind = "app"))
        val request = Request.Builder().url(base.resolve("api/pair")!!).post(body.toRequestBody(JSON)).build()
        val response = client.fetch(request, PairResponse.serializer())
        return Paired(response.token, response.device, response.version)
    }
}

private fun Response.requireSuccess() {
    if (!isSuccessful) bodyOrThrow()
}

private fun Response.bodyOrThrow(): String {
    val text = body.string()
    if (isSuccessful) return text
    val error = runCatching { MalacJson.parseToJsonElement(text) as JsonObject }.getOrNull()
        ?.get("error")?.jsonPrimitive?.content
        ?: text.ifBlank { "$code $message" }
    throw DaemonException(code, error)
}

/**
 * Sends [request] and decodes the reply. The body is read and decoded on the
 * IO dispatcher: it comes from the network and can be megabytes, and Android
 * forbids network reads on the main thread. Every failure is a
 * [DaemonException], a reply that does not decode included.
 */
internal suspend fun <T> OkHttpClient.fetch(request: Request, deserializer: DeserializationStrategy<T>): T {
    val response = newCall(request).await()
    return withContext(Dispatchers.IO) {
        response.use {
            try {
                MalacJson.decodeFromString(deserializer, it.bodyOrThrow())
            } catch (error: SerializationException) {
                throw DaemonException(it.code, "unexpected reply from the daemon: ${error.message}")
            } catch (error: IOException) {
                throw error as? DaemonException ?: DaemonException(0, error.message ?: "the connection dropped")
            }
        }
    }
}

/** Sends [request] and reads the whole body on the IO dispatcher, failing only with [DaemonException]. */
internal suspend fun OkHttpClient.download(request: Request): Download {
    val response = newCall(request).await()
    // Closed even when the download is cancelled before it reads the body.
    return response.use {
        withContext(Dispatchers.IO) {
            try {
                it.requireSuccess()
                val type = it.header("Content-Type")?.substringBefore(';')?.trim().orEmpty()
                Download(it.body.bytes(), type.ifEmpty { "application/octet-stream" })
            } catch (error: IOException) {
                throw error as? DaemonException ?: DaemonException(0, error.message ?: "the connection dropped")
            }
        }
    }
}

internal suspend fun Call.await(): Response = suspendCancellableCoroutine { continuation ->
    enqueue(
        object : Callback {
            override fun onResponse(call: Call, response: Response) {
                if (continuation.isActive) continuation.resume(response) else response.close()
            }

            override fun onFailure(call: Call, e: IOException) {
                continuation.resumeWithException(if (e is DaemonException) e else DaemonException(0, unreachable(call.request().url.host, e)))
            }
        },
    )
    continuation.invokeOnCancellation { cancel() }
}
