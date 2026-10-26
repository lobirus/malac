package io.malac.core.protocol

import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.buildClassSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonDecoder

/** Consumes a value of a type this app does not know and stands in for it. */
internal class UnknownSerializer<T>(
    private val type: String,
    private val make: (String) -> T,
) : KSerializer<T> {
    override val descriptor: SerialDescriptor = buildClassSerialDescriptor("io.malac.Unknown")

    override fun deserialize(decoder: Decoder): T {
        (decoder as JsonDecoder).decodeJsonElement()
        return make(type)
    }

    override fun serialize(encoder: Encoder, value: T) {
        throw UnsupportedOperationException("unknown values are only received")
    }
}
