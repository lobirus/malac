import MalacKit
import SwiftUI

/// A tool call as a compact row that expands to its input and output, with
/// Allow / Always / Deny while it waits for approval.
struct ToolRow: View {
    let call: ToolCall
    let session: DaemonSession
    let cwd: String?
    @State private var open: Bool?

    private static let previewLines = 24

    var body: some View {
        let title = ToolPresentation.title(call.name)
        let detail = ToolPresentation.detail(call.name, input: call.input, cwd: cwd)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { open = !expanded }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: Self.symbol(call.name)).frame(width: 18).foregroundStyle(Palette.text2)
                    Text(title.title).fontWeight(.medium)
                    if let tag = title.tag {
                        Text(tag).font(.caption2).padding(.horizontal, 5).padding(.vertical, 1).card(Palette.surface3)
                    }
                    Text(detail)
                        .foregroundStyle(Palette.text2)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 4)
                    meta
                    stateIcon
                    Image(systemName: "chevron.right")
                        .font(.caption2)
                        .foregroundStyle(Palette.text3)
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                }
                .font(.footnote)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(title.title), \(detail)")
            .accessibilityValue(stateText)
            .accessibilityHint(expanded ? "Collapses the details" : "Shows the details")

            // Who answered the approval, quietly; a denial in red, in place of its error.
            let failure = call.isError && call.decision != .deny && !expanded ? call.output.map { Format.firstLine($0) } : nil
            if call.decisionText != nil || failure != nil {
                VStack(alignment: .leading, spacing: 2) {
                    if let answer = call.decisionText {
                        Text(answer).foregroundStyle(call.decision == .deny ? Palette.red : Palette.text3)
                    }
                    if let failure {
                        Text(failure).foregroundStyle(Palette.red).lineLimit(2)
                    }
                }
                .font(.caption)
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
            if call.state == .awaitingApproval, let request = call.requestId {
                ApprovalBar(session: session, requestId: request)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
            }
            if expanded {
                Divider()
                details.padding(10)
            }
        }
        .card(call.state == .awaitingApproval ? Palette.surface : Palette.surface2)
        .overlay {
            if call.state == .awaitingApproval {
                RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Palette.amber.opacity(0.6))
            }
        }
    }

    private var expanded: Bool {
        open ?? (call.state == .awaitingApproval || (call.name == "edit" && smallEdit))
    }

    private var smallEdit: Bool {
        let stats = LineDiff.stats(diff)
        return stats.added + stats.removed <= 16
    }

    private var diff: [LineDiff.Line] {
        LineDiff.lines(from: call.input["old_string"]?.stringValue ?? "", to: call.input["new_string"]?.stringValue ?? "")
    }

    @ViewBuilder
    private var meta: some View {
        HStack(spacing: 6) {
            if call.name == "edit" {
                let stats = LineDiff.stats(diff)
                Text("+\(stats.added)").foregroundStyle(Palette.green)
                Text("−\(stats.removed)").foregroundStyle(Palette.red)
            } else if !call.isError, let output = call.output, ["read", "glob", "grep"].contains(call.name) {
                let lines = output.isEmpty ? 0 : output.trimmingCharacters(in: .newlines).components(separatedBy: "\n").count
                Text(Format.plural(lines, call.name == "read" ? "line" : "result"))
            }
            if let millis = call.millis, call.isFinished {
                Text(Format.duration(milliseconds: millis))
            }
        }
        .font(.caption.monospacedDigit())
        .foregroundStyle(Palette.text3)
    }

    @ViewBuilder
    private var stateIcon: some View {
        switch call.state {
        case .pending, .running:
            ProgressView().controlSize(.mini)
        case .awaitingApproval:
            Image(systemName: "exclamationmark.shield.fill").foregroundStyle(Palette.amber)
        case .failed, .denied:
            Image(systemName: "xmark.circle.fill").foregroundStyle(Palette.red)
        case .skipped:
            Image(systemName: "minus.circle").foregroundStyle(Palette.text3)
        case .done:
            EmptyView()
        }
    }

    private var stateText: String {
        switch call.state {
        case .pending, .running: "Running"
        case .awaitingApproval: "Needs approval"
        case .done: "Done"
        case .failed: "Failed"
        case .denied: "Denied"
        case .skipped: "Not run"
        }
    }

    @ViewBuilder
    private var details: some View {
        VStack(alignment: .leading, spacing: 10) {
            switch call.name {
            case "bash":
                if let description = call.input["description"]?.stringValue {
                    Text(description).font(.footnote).foregroundStyle(Palette.text2)
                }
                DetailSection(title: "Command", copy: call.input["command"]?.stringValue) {
                    Output(text: "$ " + (call.input["command"]?.stringValue ?? ""), lines: Self.previewLines)
                }
            case "edit":
                DiffView(lines: diff)
            case "write":
                DetailSection(title: Format.relativePath(call.input["path"]?.stringValue ?? "", to: cwd), copy: call.input["content"]?.stringValue) {
                    Output(text: call.input["content"]?.stringValue ?? "", lines: Self.previewLines)
                }
            case "read", "glob", "grep", "web_fetch":
                EmptyView()
            default:
                DetailSection(title: "Input", copy: call.input.prettyText) {
                    Output(text: call.input.prettyText, lines: Self.previewLines)
                }
            }
            if let output = call.output, showsOutput {
                DetailSection(title: call.isError ? "Error" : "Output", copy: output) {
                    Output(text: output.isEmpty ? "(no output)" : output, lines: Self.previewLines, error: call.isError)
                }
            } else if !call.isFinished {
                Text(call.state == .awaitingApproval ? "Waiting for approval" : call.state == .skipped ? "Not run" : "Running")
                    .font(.footnote).foregroundStyle(Palette.text3)
            }
            if !call.images.isEmpty {
                ScrollView(.horizontal) {
                    HStack {
                        ForEach(Array(call.images.enumerated()), id: \.offset) { index, image in
                            Thumbnail(image: image, name: call.images.count == 1 ? "\(ToolPresentation.title(call.name).title) image" : "\(ToolPresentation.title(call.name).title) image \(index + 1)")
                        }
                    }
                }
            }
        }
    }

    private var showsOutput: Bool {
        if call.isError { return true }
        return call.name != "edit" && call.name != "write"
    }

    static func symbol(_ tool: String) -> String {
        switch tool {
        case "bash": "terminal"
        case "read": "doc.text"
        case "write": "doc.badge.plus"
        case "edit": "pencil"
        case "glob": "folder"
        case "grep": "magnifyingglass"
        case "web_fetch": "globe"
        case "agent_wait": "hourglass"
        case let name where name.hasPrefix("browser_"): "safari"
        case let name where name.hasPrefix("computer_"): "desktopcomputer"
        case let name where name.hasPrefix("memory_"): "bookmark"
        case let name where name.hasPrefix("job_"): "gearshape.2"
        case let name where name.contains("__"): "puzzlepiece.extension"
        default: "wrench.and.screwdriver"
        }
    }
}

private struct DetailSection<Content: View>: View {
    let title: String
    let copy: String?
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(title).font(.caption.weight(.semibold)).foregroundStyle(Palette.text2)
                Spacer()
                if let copy { CopyButton(text: copy) }
            }
            content()
        }
    }
}

/// Monospaced output, clipped to its first lines until "Show all".
private struct Output: View {
    let text: String
    let lines: Int
    var error = false
    @State private var all = false

    var body: some View {
        let split = text.components(separatedBy: "\n")
        let long = split.count > lines * 2
        VStack(alignment: .leading, spacing: 6) {
            ScrollView(.horizontal) {
                Text(long && !all ? split.prefix(lines).joined(separator: "\n") : text)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(error ? Palette.red : Palette.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(8)
            }
            .card(Palette.surface)
            if long {
                Button(all ? "Show less" : "Show all \(split.count) lines") { all.toggle() }
                    .font(.caption)
                    .tint(Palette.green)
            }
        }
    }
}

private struct DiffView: View {
    let lines: [LineDiff.Line]

    var body: some View {
        ScrollView(.horizontal) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(LineDiff.rows(lines).enumerated()), id: \.offset) { _, row in
                    switch row {
                    case .gap(let count):
                        Text(Format.plural(count, "unchanged line"))
                            .font(.caption2)
                            .foregroundStyle(Palette.text3)
                            .padding(.vertical, 2)
                            .padding(.horizontal, 8)
                    case .line(let line):
                        HStack(spacing: 6) {
                            Text(line.op == .added ? "+" : line.op == .removed ? "−" : " ")
                                .foregroundStyle(line.op == .added ? Palette.green : Palette.red)
                                .accessibilityHidden(true)
                            Text(line.text.isEmpty ? " " : line.text)
                        }
                        .font(.system(.caption, design: .monospaced))
                        .padding(.horizontal, 8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(line.op == .added ? Palette.diffAdded : line.op == .removed ? Palette.diffRemoved : .clear)
                        .accessibilityLabel((line.op == .added ? "Added: " : line.op == .removed ? "Removed: " : "") + line.text)
                    }
                }
            }
            .fixedSize(horizontal: true, vertical: false)
            .padding(.vertical, 4)
        }
        .card(Palette.surface)
    }
}

/// Allow, Always and Deny for one permission request.
struct ApprovalBar: View {
    let session: DaemonSession
    let requestId: String

    var body: some View {
        let busy = session.answering.contains(requestId)
        HStack(spacing: 8) {
            Button("Allow") { answer(.allow) }
                .buttonStyle(.borderedProminent)
                .tint(Palette.primary)
                .foregroundStyle(Palette.onPrimary)
            Button("Always") { answer(.allowSession) }
                .buttonStyle(.bordered)
                .tint(Palette.text2)
                .accessibilityHint("Allows this and similar calls for the rest of the session")
            Button("Deny", role: .destructive) { answer(.deny) }
                .buttonStyle(.bordered)
                .tint(Palette.red)
            if busy { ProgressView().controlSize(.small) }
        }
        .controlSize(.small)
        .disabled(busy || !session.isOnline)
        .sensoryFeedback(.success, trigger: busy) { _, new in new }
    }

    private func answer(_ decision: Decision) {
        Task { await session.approve(requestId, decision) }
    }
}
