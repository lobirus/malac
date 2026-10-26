import Foundation

/// Text helpers shared by the views, matching the web UI's wording.
public enum Format {
    public static func truncate(_ text: String, _ max: Int) -> String {
        text.count > max ? String(text.prefix(max - 1)) + "…" : text
    }

    public static func firstLine(_ text: String?) -> String {
        let trimmed = (text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? ""
    }

    public static func baseName(_ path: String) -> String {
        path.split(separator: "/").last.map(String.init) ?? path
    }

    public static func plural(_ count: Int, _ word: String, _ many: String? = nil) -> String {
        "\(count) \(count == 1 ? word : many ?? word + "s")"
    }

    /// `path` relative to `cwd` when it is inside it.
    public static func relativePath(_ path: String, to cwd: String?) -> String {
        guard let cwd, !cwd.isEmpty, path.hasPrefix(cwd + "/") else { return path }
        return String(path.dropFirst(cwd.count + 1))
    }

    /// Paths under `home` read as `~/…`.
    public static func displayPath(_ path: String, home: String?) -> String {
        guard var home, !home.isEmpty else { return path }
        while home.hasSuffix("/") { home.removeLast() }
        guard !home.isEmpty, path == home || path.hasPrefix(home + "/") else { return path }
        return "~" + path.dropFirst(home.count)
    }

    /// 950, 7k, 7.2k, 1.4M.
    public static func tokens(_ count: Int) -> String {
        if count < 1000 { return String(count) }
        if count < 1_000_000 { return oneDecimal(Double(count) / 1000) + "k" }
        return oneDecimal(Double(count) / 1_000_000) + "M"
    }

    /// $0.0046, $1.20; empty when the provider reports no cost.
    public static func cost(_ cost: Double) -> String {
        if cost <= 0 { return "" }
        return String(format: cost < 0.01 ? "$%.4f" : "$%.2f", locale: Locale(identifier: "en_US_POSIX"), cost)
    }

    /// 640ms, 1.5s, 3m 04s, 2h 05m.
    public static func duration(milliseconds: Int) -> String {
        if milliseconds < 1000 { return "\(max(0, milliseconds))ms" }
        if milliseconds < 60_000 { return oneDecimal(Double(milliseconds) / 1000) + "s" }
        let seconds = Int((Double(milliseconds) / 1000).rounded())
        if seconds < 3600 { return String(format: "%dm %02ds", seconds / 60, seconds % 60) }
        return String(format: "%dh %02dm", seconds / 3600, (seconds % 3600) / 60)
    }

    public static func duration(from start: Date, to end: Date) -> String {
        duration(milliseconds: Int(end.timeIntervalSince(start) * 1000))
    }

    /// "7k in · 20 out", plus cached tokens when there are any.
    public static func usage(_ usage: Usage) -> String {
        var parts = ["\(tokens(usage.inputTokens)) in", "\(tokens(usage.outputTokens)) out"]
        if usage.cacheReadTokens > 0 { parts.append("\(tokens(usage.cacheReadTokens)) cached") }
        return parts.joined(separator: " · ")
    }

    private static func oneDecimal(_ value: Double) -> String {
        let text = String(format: "%.1f", locale: Locale(identifier: "en_US_POSIX"), value)
        return text.hasSuffix(".0") ? String(text.dropLast(2)) : text
    }
}
