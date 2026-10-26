import Foundation

/// The block structure of the Markdown models write. Inline text (emphasis,
/// code spans, links) is left as Markdown for the platform to render.
public indirect enum MarkdownBlock: Hashable, Sendable {
    case heading(level: Int, text: String)
    /// Line breaks inside a paragraph are kept, as in chat.
    case paragraph(String)
    case code(language: String, code: String)
    case quote([MarkdownBlock])
    case list(MarkdownList)
    case table(MarkdownTable)
    case rule
}

public struct MarkdownList: Hashable, Sendable {
    public var ordered: Bool
    public var start: Int
    public var items: [MarkdownListItem]
    /// Items separated by blank lines get paragraph spacing.
    public var loose: Bool
}

public struct MarkdownListItem: Hashable, Sendable {
    public var blocks: [MarkdownBlock]
    /// Nil for a plain item, else whether its box is checked.
    public var task: Bool?
}

public struct MarkdownTable: Hashable, Sendable {
    public enum Alignment: Hashable, Sendable { case leading, center, trailing }

    public var header: [String]
    public var alignments: [Alignment]
    public var rows: [[String]]
}

public enum Markdown {
    public static func parse(_ source: String) -> [MarkdownBlock] {
        let lines = source.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
            .components(separatedBy: "\n")
        return Parser(lines: lines).blocks()
    }
}

private struct Parser {
    let lines: [String]

    private static let fence = #/^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)/#
    private static let heading = #/^ {0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/#
    private static let rule = #/^ {0,3}([-*_])(?:\s*\1){2,}\s*$/#
    private static let quote = #/^ {0,3}> ?/#
    private static let listItem = #/^( *)([-*+]|\d{1,9}[.)])\s+(.*)$/#
    private static let task = #/^\[([ xX])\]\s+/#
    private static let tableDivider = #/^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$/#

    func blocks() -> [MarkdownBlock] {
        var out: [MarkdownBlock] = []
        var i = 0
        while i < lines.count {
            if isBlank(lines[i]) {
                i += 1
                continue
            }
            let (block, next) = fencedCode(i) ?? heading(i) ?? rule(i) ?? quote(i) ?? table(i) ?? list(i) ?? paragraph(i)
            out.append(block)
            i = next
        }
        return out
    }

    private func fencedCode(_ start: Int) -> (MarkdownBlock, Int)? {
        guard let open = lines[start].prefixMatch(of: Self.fence) else { return nil }
        let marker = open.1.first!
        let length = open.1.count
        var body: [String] = []
        var i = start + 1
        while i < lines.count, !closes(lines[i], marker: marker, length: length) {
            body.append(lines[i])
            i += 1
        }
        let language = String(open.2).filter { $0.isLetter || $0.isNumber || "+#.-_".contains($0) }.lowercased()
        // An unclosed fence runs to the end, which is what a streaming answer looks like.
        return (.code(language: language, code: body.joined(separator: "\n")), i + 1)
    }

    private func closes(_ line: String, marker: Character, length: Int) -> Bool {
        let trimmed = line.drop { $0 == " " }
        guard line.count - trimmed.count <= 3 else { return false }
        let run = trimmed.prefix { $0 == marker }
        return run.count >= length && trimmed.dropFirst(run.count).allSatisfy(\.isWhitespace)
    }

    private func heading(_ i: Int) -> (MarkdownBlock, Int)? {
        guard let match = lines[i].wholeMatch(of: Self.heading) else { return nil }
        return (.heading(level: match.1.count, text: String(match.2)), i + 1)
    }

    private func rule(_ i: Int) -> (MarkdownBlock, Int)? {
        lines[i].wholeMatch(of: Self.rule) == nil ? nil : (.rule, i + 1)
    }

    private func quote(_ start: Int) -> (MarkdownBlock, Int)? {
        guard lines[start].prefixMatch(of: Self.quote) != nil else { return nil }
        var body: [String] = []
        var i = start
        while i < lines.count, let marker = lines[i].prefixMatch(of: Self.quote) {
            body.append(String(lines[i][marker.range.upperBound...]))
            i += 1
        }
        return (.quote(Parser(lines: body).blocks()), i)
    }

    private func isTableStart(_ i: Int) -> Bool {
        lines[i].contains("|") && i + 1 < lines.count && lines[i + 1].contains("|")
            && lines[i + 1].wholeMatch(of: Self.tableDivider) != nil
    }

    private func table(_ start: Int) -> (MarkdownBlock, Int)? {
        guard isTableStart(start) else { return nil }
        let header = Self.cells(lines[start])
        let dividers = Self.cells(lines[start + 1])
        var rows: [[String]] = []
        var i = start + 2
        while i < lines.count, lines[i].contains("|"), !isBlank(lines[i]) {
            let cells = Self.cells(lines[i])
            rows.append(header.indices.map { $0 < cells.count ? cells[$0] : "" })
            i += 1
        }
        let alignments = header.indices.map { $0 < dividers.count ? Self.alignment(dividers[$0]) : .leading }
        return (.table(MarkdownTable(header: header, alignments: alignments, rows: rows)), i)
    }

    private func list(_ start: Int) -> (MarkdownBlock, Int)? {
        guard let first = lines[start].wholeMatch(of: Self.listItem) else { return nil }
        let indent = first.1.count
        let ordered = first.2.first!.isNumber
        func sameList(_ line: String) -> Bool {
            guard let match = line.wholeMatch(of: Self.listItem) else { return false }
            return match.1.count == indent && match.2.first!.isNumber == ordered
        }

        var items: [(lines: [String], contentIndent: Int)] = []
        var loose = false
        var i = start
        while i < lines.count {
            let line = lines[i]
            if sameList(line), let match = line.wholeMatch(of: Self.listItem) {
                let content = String(match.3)
                items.append(([content], line.count - content.count))
                i += 1
                continue
            }
            if line.wholeMatch(of: Self.listItem) != nil, indentation(line) <= indent { break }
            if isBlank(line) {
                guard i + 1 < lines.count else { break }
                let next = lines[i + 1]
                if indentation(next) <= indent, !sameList(next) { break }
                items[items.count - 1].lines.append("")
                loose = true
            } else if indentation(line) > indent {
                let drop = min(indentation(line), items[items.count - 1].contentIndent)
                items[items.count - 1].lines.append(String(line.dropFirst(drop)))
            } else if startsBlock(line) {
                break
            } else {
                items[items.count - 1].lines.append(line.trimmingCharacters(in: .whitespaces))
            }
            i += 1
        }
        let number = Int(first.2.prefix { $0.isNumber }) ?? 1
        let parsed = items.map { item -> MarkdownListItem in
            var body = item.lines
            var task: Bool?
            if let box = body[0].prefixMatch(of: Self.task) {
                task = box.1 != " "
                body[0] = String(body[0][box.range.upperBound...])
            }
            return MarkdownListItem(blocks: Parser(lines: body).blocks(), task: task)
        }
        return (.list(MarkdownList(ordered: ordered, start: ordered ? number : 1, items: parsed, loose: loose)), i)
    }

    private func paragraph(_ start: Int) -> (MarkdownBlock, Int) {
        var body = [lines[start].trimmingCharacters(in: .whitespaces)]
        var i = start + 1
        while i < lines.count, !isBlank(lines[i]), !startsBlock(lines[i]), !isTableStart(i) {
            body.append(lines[i].trimmingCharacters(in: .whitespaces))
            i += 1
        }
        return (.paragraph(body.joined(separator: "\n")), i)
    }

    private func startsBlock(_ line: String) -> Bool {
        line.prefixMatch(of: Self.fence) != nil || line.wholeMatch(of: Self.heading) != nil
            || line.wholeMatch(of: Self.rule) != nil || line.prefixMatch(of: Self.quote) != nil
            || line.wholeMatch(of: Self.listItem) != nil
    }

    private func isBlank(_ line: String) -> Bool {
        line.allSatisfy(\.isWhitespace)
    }

    private func indentation(_ line: String) -> Int {
        line.prefix { $0 == " " || $0 == "\t" }.count
    }

    /// The cells of a table row, split on pipes that are not escaped.
    private static func cells(_ line: String) -> [String] {
        var row = line.trimmingCharacters(in: .whitespaces)
        if row.hasPrefix("|") { row.removeFirst() }
        if row.hasSuffix("|"), !row.hasSuffix("\\|") { row.removeLast() }
        var cells: [String] = []
        var cell = ""
        var escaped = false
        for character in row {
            if escaped {
                cell.append(character == "|" ? "|" : "\\\(character)")
                escaped = false
            } else if character == "\\" {
                escaped = true
            } else if character == "|" {
                cells.append(cell.trimmingCharacters(in: .whitespaces))
                cell = ""
            } else {
                cell.append(character)
            }
        }
        if escaped { cell.append("\\") }
        cells.append(cell.trimmingCharacters(in: .whitespaces))
        return cells
    }

    private static func alignment(_ divider: String) -> MarkdownTable.Alignment {
        switch (divider.hasPrefix(":"), divider.hasSuffix(":")) {
        case (true, true): .center
        case (false, true): .trailing
        default: .leading
        }
    }
}
