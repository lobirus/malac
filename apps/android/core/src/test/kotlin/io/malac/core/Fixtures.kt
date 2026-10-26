package io.malac.core

import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.ServerMessage
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/** The protocol fixtures in `apps/fixtures`, shared with the iOS app. */
object Fixtures {
    const val MAIN = "01a176fa-99d2-7014-9be0-383c2c41ddc3"
    const val READER = "01a176fa-a30f-7178-9dba-d02eeede0201"
    const val SECOND = "01a176fa-c603-748e-85a9-2971e96f0c17"
    const val TAB = "01a176fa-99d0-733d-bec9-e9d000301f35"
    const val WORKSPACE = "01a176fa-99d0-733d-bec9-e9cf16452a88"

    fun text(name: String): String =
        checkNotNull(Fixtures::class.java.classLoader.getResource(name)) { "missing fixture $name" }.readText()

    /** Every line of the recorded session, raw: `{dir: "in" | "out", msg}`. */
    val sessionLines: List<JsonObject> by lazy {
        text("session.jsonl").lines().filter { it.isNotBlank() }.map { MalacJson.parseToJsonElement(it).jsonObject }
    }

    /** What the daemon sent, in order. */
    val received: List<String> by lazy {
        sessionLines.filter { it["dir"]?.jsonPrimitive?.content == "in" }.map { it.getValue("msg").toString() }
    }

    val receivedMessages: List<ServerMessage> by lazy {
        received.map { MalacJson.decodeFromString(ServerMessage.serializer(), it) }
    }
}
