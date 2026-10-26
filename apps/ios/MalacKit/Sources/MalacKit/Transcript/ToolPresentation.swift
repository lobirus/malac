import Foundation

/// How tool calls read in compact rows, as in the web UI.
public enum ToolPresentation {
    /// A title and an optional tag such as an MCP server or "browser".
    public static func title(_ name: String) -> (title: String, tag: String?) {
        if let title = titles[name] { return (title, nil) }
        if let range = name.range(of: "__") {
            let server = String(name[..<range.lowerBound])
            let tool = String(name[range.upperBound...]).replacingOccurrences(of: "_", with: " ")
            return (tool, server)
        }
        let tag = ["browser", "computer", "memory"].first { name.hasPrefix($0 + "_") }
        return (humanize(name), tag)
    }

    /// The one-line detail of a call, with paths relative to the agent's directory.
    public static func detail(_ name: String, input: JSONValue, cwd: String?) -> String {
        switch name {
        case "read":
            let path = Format.relativePath(input["path"]?.stringValue ?? "", to: cwd)
            guard let offset = input["offset"]?.intValue, offset != 0 else { return path }
            return path + ":\(offset)" + (input["limit"]?.intValue.map { "+\($0)" } ?? "")
        case "write", "edit":
            return Format.relativePath(input["path"]?.stringValue ?? "", to: cwd)
        case "glob", "grep":
            let pattern = input["pattern"]?.stringValue ?? ""
            guard let path = input["path"]?.stringValue else { return pattern }
            return "\(pattern) in \(Format.relativePath(path, to: cwd))"
        case "bash":
            return Format.firstLine(input["command"]?.stringValue)
        default:
            return summary(name, input: input)
        }
    }

    public static func summary(_ name: String, input: JSONValue) -> String {
        guard let object = input.objectValue else { return "" }
        switch name {
        case "agent_spawn":
            let label = input["label"]?.stringValue ?? input["agent"]?.stringValue ?? "agent"
            return "\(label): \(Format.firstLine(input["task"]?.stringValue))"
        case "todo":
            return Format.plural(input["items"]?.arrayValue?.count ?? 0, "item")
        case "agent_wait":
            let ids = input["ids"]?.arrayValue?.count ?? 0
            return ids > 0 ? Format.plural(ids, "agent") : "all subagents"
        default:
            break
        }
        if let text = summaryKeys.lazy.compactMap({ object[$0]?.stringValue }).first {
            return Format.truncate(Format.firstLine(text), 200)
        }
        let json = input.compactText
        return json == "{}" ? "" : Format.truncate(json, 200)
    }

    /// The child id in an `agent_spawn` result: "Report from `x` (id <uuid>)".
    public static func spawnedAgentId(_ output: String) -> String? {
        guard let match = output.firstMatch(of: #/\(id ([0-9a-f-]{36})\)/#) else { return nil }
        return String(match.1)
    }

    /// The report in an `agent_spawn` result, without its header.
    public static func spawnReport(_ output: String) -> String {
        guard let match = output.prefixMatch(of: #/Report from `[^`]*` \(id [0-9a-f-]+\):\s*/#) else { return output }
        return String(output[match.range.upperBound...])
    }

    private static let titles = [
        "bash": "Bash", "read": "Read", "write": "Write", "edit": "Edit", "glob": "Glob", "grep": "Grep",
        "web_fetch": "Fetch", "agent_wait": "Wait for subagents",
    ]

    private static let summaryKeys = ["command", "path", "pattern", "url", "query", "name", "text", "id", "message"]

    private static func humanize(_ name: String) -> String {
        var text = name
        for prefix in ["browser_", "computer_", "memory_", "agent_"] where text.hasPrefix(prefix) {
            text.removeFirst(prefix.count)
            break
        }
        text = text.replacingOccurrences(of: "_", with: " ")
        return text.prefix(1).uppercased() + text.dropFirst()
    }
}
