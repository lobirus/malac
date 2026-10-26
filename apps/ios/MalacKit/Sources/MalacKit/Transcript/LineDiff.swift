import Foundation

/// A line diff of an `edit` call's `old_string` against its `new_string`.
public enum LineDiff {
    public enum Op: Hashable, Sendable { case same, added, removed }

    public struct Line: Hashable, Sendable {
        public var op: Op
        public var text: String
    }

    /// What a diff view shows: changed lines with some context, and gaps.
    public enum Row: Hashable, Sendable {
        case line(Line)
        case gap(Int)
    }

    /// Longest common subsequence after trimming the common prefix and
    /// suffix; past `maxCells` the middle is shown as removed then added.
    public static func lines(from before: String, to after: String, maxCells: Int = 400_000) -> [Line] {
        let a = before.isEmpty ? [] : before.components(separatedBy: "\n")
        let b = after.isEmpty ? [] : after.components(separatedBy: "\n")
        var start = 0
        while start < a.count, start < b.count, a[start] == b[start] { start += 1 }
        var endA = a.count
        var endB = b.count
        while endA > start, endB > start, a[endA - 1] == b[endB - 1] {
            endA -= 1
            endB -= 1
        }
        let midA = Array(a[start ..< endA])
        let midB = Array(b[start ..< endB])
        var result = a[..<start].map { Line(op: .same, text: $0) }
        if midA.count * midB.count > maxCells {
            result += midA.map { Line(op: .removed, text: $0) } + midB.map { Line(op: .added, text: $0) }
        } else {
            let n = midA.count
            let m = midB.count
            var table = [[Int]](repeating: [Int](repeating: 0, count: m + 1), count: n + 1)
            for i in stride(from: n - 1, through: 0, by: -1) {
                for j in stride(from: m - 1, through: 0, by: -1) {
                    table[i][j] = midA[i] == midB[j] ? table[i + 1][j + 1] + 1 : max(table[i + 1][j], table[i][j + 1])
                }
            }
            var i = 0
            var j = 0
            while i < n || j < m {
                if i < n, j < m, midA[i] == midB[j] {
                    result.append(Line(op: .same, text: midA[i]))
                    i += 1
                    j += 1
                } else if i < n, j == m || table[i + 1][j] >= table[i][j + 1] {
                    result.append(Line(op: .removed, text: midA[i]))
                    i += 1
                } else {
                    result.append(Line(op: .added, text: midB[j]))
                    j += 1
                }
            }
        }
        result += a[endA...].map { Line(op: .same, text: $0) }
        return result
    }

    /// Unchanged lines more than `context` lines away from a change become gaps.
    public static func rows(_ lines: [Line], context: Int = 3) -> [Row] {
        let changed = lines.indices.filter { lines[$0].op != .same }
        var near = Set<Int>()
        for index in changed {
            for k in max(0, index - context) ... min(lines.count - 1, index + context) { near.insert(k) }
        }
        var rows: [Row] = []
        var hidden = 0
        for (index, line) in lines.enumerated() {
            if line.op == .same, !near.contains(index) {
                hidden += 1
                continue
            }
            if hidden > 0 { rows.append(.gap(hidden)) }
            hidden = 0
            rows.append(.line(line))
        }
        if hidden > 0 { rows.append(.gap(hidden)) }
        return rows
    }

    public static func stats(_ lines: [Line]) -> (added: Int, removed: Int) {
        (lines.filter { $0.op == .added }.count, lines.filter { $0.op == .removed }.count)
    }
}
