package io.malac.core.protocol

import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonContentPolymorphicSerializer
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

@Serializable
enum class Role {
    @SerialName("user") User,
    @SerialName("assistant") Assistant,
}

@Serializable
data class Message(
    val role: Role = Role.User,
    val content: List<Block> = emptyList(),
) {
    /** Concatenated text blocks, ignoring thinking and tool traffic. */
    val text: String get() = content.filterIsInstance<Block.Text>().joinToString("\n") { it.text }
}

@Serializable(with = BlockSerializer::class)
sealed interface Block {
    @Serializable
    data class Text(val text: String = "") : Block

    @Serializable
    data class Image(
        @SerialName("media_type") val mediaType: String = "",
        /** Base64. */
        val data: String = "",
    ) : Block

    @Serializable
    data class Thinking(val text: String = "") : Block

    @Serializable
    data class ToolUse(
        val id: String,
        val name: String,
        val input: JsonElement = JsonObject(emptyMap()),
    ) : Block

    @Serializable
    data class ToolResult(
        @SerialName("tool_use_id") val toolUseId: String,
        val content: List<Block> = emptyList(),
        @SerialName("is_error") val isError: Boolean = false,
    ) : Block {
        /** The result as text, with images as placeholders the way the daemon previews them. */
        val text: String
            get() = content.joinToString("") {
                when (it) {
                    is Text -> it.text
                    is Image -> "[${it.mediaType} image]"
                    else -> ""
                }
            }
    }

    /** A block type this app does not know yet. */
    data class Unknown(val type: String) : Block
}

internal object BlockSerializer : JsonContentPolymorphicSerializer<Block>(Block::class) {
    override fun selectDeserializer(element: JsonElement): DeserializationStrategy<Block> =
        when (val type = element.jsonObject["type"]?.jsonPrimitive?.content) {
            "text" -> Block.Text.serializer()
            "image" -> Block.Image.serializer()
            "thinking" -> Block.Thinking.serializer()
            "tool_use" -> Block.ToolUse.serializer()
            "tool_result" -> Block.ToolResult.serializer()
            else -> UnknownSerializer(type.orEmpty()) { Block.Unknown(it) }
        }
}
