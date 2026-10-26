package io.malac.core.protocol

import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import java.time.Instant
import java.time.format.DateTimeFormatter

/**
 * The daemon is newer or older than the app more often than not, so decoding
 * ignores fields it does not know and falls back to defaults for enum values
 * it does not know.
 */
val MalacJson: Json = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    coerceInputValues = true
    isLenient = true
}

/** RFC 3339 timestamps as chrono writes them, with up to nine fractional digits. */
object InstantSerializer : KSerializer<Instant> {
    override val descriptor = PrimitiveSerialDescriptor("io.malac.Instant", PrimitiveKind.STRING)

    override fun deserialize(decoder: Decoder): Instant =
        DateTimeFormatter.ISO_OFFSET_DATE_TIME.parse(decoder.decodeString(), Instant::from)

    override fun serialize(encoder: Encoder, value: Instant) {
        encoder.encodeString(value.toString())
    }
}

typealias Timestamp = @kotlinx.serialization.Serializable(with = InstantSerializer::class) Instant
