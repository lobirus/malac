package io.malac.core.transcript

import io.malac.core.format.firstLine
import io.malac.core.format.plural
import io.malac.core.format.relativePath
import io.malac.core.format.truncate
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull

/** How a tool call reads in a compact row: a title, an optional tag (MCP server, browser...) and a detail. */
data class ToolTitle(val title: String, val tag: String?)

private val TOOL_TITLES = mapOf(
    "bash" to "Bash",
    "read" to "Read",
    "write" to "Write",
    "edit" to "Edit",
    "glob" to "Glob",
    "grep" to "Grep",
    "web_fetch" to "Fetch",
    "agent_wait" to "Wait for subagents",
)

private val SUMMARY_KEYS = listOf("command", "path", "pattern", "url", "query", "name", "text", "id", "message")

private val TAGGED_PREFIXES = listOf("browser", "computer", "memory")

fun toolTitle(name: String): ToolTitle {
    TOOL_TITLES[name]?.let { return ToolTitle(it, null) }
    if ("__" in name) {
        val server = name.substringBefore("__")
        return ToolTitle(name.substringAfter("__").replace('_', ' '), server)
    }
    val tag = TAGGED_PREFIXES.firstOrNull { name.startsWith("${it}_") }
    return ToolTitle(humanize(name), tag)
}

private fun humanize(name: String): String {
    val text = name.replace(Regex("^(browser|computer|memory|agent)_"), "").replace('_', ' ')
    return text.replaceFirstChar { it.uppercaseChar() }
}

internal fun JsonElement?.string(key: String): String? =
    ((this as? JsonObject)?.get(key) as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

internal fun JsonElement?.int(key: String): Int? = ((this as? JsonObject)?.get(key) as? JsonPrimitive)?.intOrNull

internal fun JsonElement?.bool(key: String): Boolean = ((this as? JsonObject)?.get(key) as? JsonPrimitive)?.booleanOrNull == true

/** The one-line detail of a tool call, relative to the agent's directory. */
fun describeTool(name: String, input: JsonElement, cwd: String?): String = when (name) {
    "read" -> {
        val offset = input.int("offset")
        val limit = input.int("limit")
        val range = if (offset != null && offset != 0) ":$offset${if (limit != null) "+$limit" else ""}" else ""
        relativePath(input.string("path").orEmpty(), cwd) + range
    }
    "write", "edit" -> relativePath(input.string("path").orEmpty(), cwd)
    "glob", "grep" -> {
        val path = input.string("path")
        input.string("pattern").orEmpty() + if (path != null) " in ${relativePath(path, cwd)}" else ""
    }
    "bash" -> firstLine(input.string("command"))
    else -> summarizeInput(name, input)
}

fun summarizeInput(name: String, input: JsonElement): String {
    if (input !is JsonObject) return ""
    when (name) {
        "agent_spawn" -> return "${input.string("label") ?: input.string("agent") ?: "agent"}: ${firstLine(input.string("task"))}"
        "todo" -> return plural((input["items"] as? JsonArray)?.size ?: 0, "item")
        "agent_wait" -> {
            val ids = (input["ids"] as? JsonArray)?.size ?: 0
            return if (ids > 0) plural(ids, "agent") else "all subagents"
        }
    }
    SUMMARY_KEYS.firstNotNullOfOrNull { input.string(it) }?.let { return truncate(firstLine(it), 200) }
    val json = input.toString()
    return if (json == "{}") "" else truncate(json, 200)
}

/** The child id in an `agent_spawn` result: "Report from `x` (id <uuid>)". */
internal fun spawnedAgentId(output: String): String? =
    Regex("""\(id ([0-9a-f-]{36})\)""").find(output)?.groupValues?.get(1)

/** The report in an `agent_spawn` result, without its header. */
internal fun spawnReport(output: String): String {
    val header = Regex("""^Report from `[^`]*` \(id [0-9a-f-]+\):\s*""").find(output) ?: return output
    return output.substring(header.range.last + 1)
}
