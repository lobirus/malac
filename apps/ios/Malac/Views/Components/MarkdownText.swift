import MalacKit
import SwiftUI

/// Markdown as native text: MalacKit parses the blocks, Foundation the inline
/// emphasis, code spans and links.
struct MarkdownText: View {
    let blocks: [MarkdownBlock]

    init(_ markdown: String) {
        blocks = Markdown.parse(markdown)
    }

    var body: some View {
        MarkdownBlocks(blocks: blocks)
            .tint(Palette.green)
            .textSelection(.enabled)
    }

    static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible
        )
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

private struct MarkdownBlocks: View {
    let blocks: [MarkdownBlock]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                BlockView(block: block)
            }
        }
    }
}

private struct BlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case let .heading(level, text):
            Text(MarkdownText.inline(text))
                .font(level == 1 ? .title3 : level == 2 ? .headline : .subheadline)
                .fontWeight(.semibold)
                .accessibilityAddTraits(.isHeader)
        case .paragraph(let text):
            Text(MarkdownText.inline(text))
        case let .code(language, code):
            CodeBlock(code: code, language: language)
        case .quote(let blocks):
            HStack(alignment: .top, spacing: 10) {
                Rectangle().fill(Palette.border).frame(width: 3)
                MarkdownBlocks(blocks: blocks).foregroundStyle(Palette.text2)
            }
            .fixedSize(horizontal: false, vertical: true)
        case .list(let list):
            ListView(list: list)
        case .table(let table):
            TableView(table: table)
        case .rule:
            Divider()
        }
    }
}

private struct ListView: View {
    let list: MarkdownList

    var body: some View {
        VStack(alignment: .leading, spacing: list.loose ? 10 : 4) {
            ForEach(Array(list.items.enumerated()), id: \.offset) { index, item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    marker(index: index, item: item)
                    MarkdownBlocks(blocks: item.blocks)
                }
            }
        }
    }

    @ViewBuilder
    private func marker(index: Int, item: MarkdownListItem) -> some View {
        if let done = item.task {
            Image(systemName: done ? "checkmark.square" : "square")
                .foregroundStyle(done ? Palette.green : Palette.text3)
                .accessibilityLabel(done ? "Done" : "Not done")
        } else if list.ordered {
            Text("\(list.start + index).").monospacedDigit().foregroundStyle(Palette.text2)
        } else {
            Text("•").foregroundStyle(Palette.text2).accessibilityHidden(true)
        }
    }
}

private struct TableView: View {
    let table: MarkdownTable

    var body: some View {
        ScrollView(.horizontal) {
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
                GridRow {
                    ForEach(Array(table.header.enumerated()), id: \.offset) { column, cell in
                        Text(MarkdownText.inline(cell)).fontWeight(.semibold).gridColumnAlignment(alignment(column))
                    }
                }
                Divider()
                ForEach(Array(table.rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                            Text(MarkdownText.inline(cell))
                        }
                    }
                }
            }
            .font(.callout)
            .padding(10)
        }
        .card(Palette.surface)
    }

    private func alignment(_ column: Int) -> HorizontalAlignment {
        switch table.alignments[column] {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }
}

/// Monospaced code that scrolls sideways, with its language and a copy button.
struct CodeBlock: View {
    let code: String
    var language = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language.isEmpty ? "code" : language)
                    .font(.caption)
                    .foregroundStyle(Palette.text3)
                Spacer()
                CopyButton(text: code)
            }
            .padding(.horizontal, 10)
            .padding(.top, 6)
            ScrollView(.horizontal) {
                Text(code)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(10)
            }
        }
        .card()
    }
}

struct CopyButton: View {
    let text: String
    @State private var copied = false

    var body: some View {
        Button {
            UIPasteboard.general.string = text
            copied = true
            Task {
                try? await Task.sleep(for: .seconds(1.5))
                copied = false
            }
        } label: {
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .font(.caption)
                .foregroundStyle(copied ? Palette.green : Palette.text3)
                .frame(minWidth: 32, minHeight: 32)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(copied ? "Copied" : "Copy")
    }
}
