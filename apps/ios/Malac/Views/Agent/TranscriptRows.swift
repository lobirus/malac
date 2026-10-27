import MalacKit
import SwiftUI

struct TranscriptRow: View {
    let item: TranscriptItem
    let session: DaemonSession
    let cwd: String?

    var body: some View {
        switch item {
        case .user(let message):
            UserBubble(message: message)
        case .assistant(let text):
            VStack(alignment: .leading, spacing: 4) {
                MarkdownText(text.markdown)
                if text.interrupted {
                    Text("Interrupted").font(.caption).foregroundStyle(Palette.text3)
                }
            }
        case .thinking(_, let text):
            Disclosure(title: "Thinking", icon: "lightbulb", preview: Format.truncate(Format.firstLine(text), 80)) {
                Text(text).font(.callout).foregroundStyle(Palette.text2).textSelection(.enabled)
            }
        case .recalled(_, let text):
            Disclosure(title: "Recalled memories", icon: "bookmark") {
                Text(text).font(.callout).foregroundStyle(Palette.text2)
            }
        case .tool(let call):
            if let shared = call.shared {
                SharedFilesCard(shared: shared, session: session)
            } else {
                ToolRow(call: call, session: session, cwd: cwd)
            }
        case .subagent(let call):
            SubagentCard(call: call, session: session)
        case .plan(let plan):
            PlanCard(plan: plan)
        case .compacted(_, let summary):
            Disclosure(title: "Conversation compacted", icon: "arrow.down.right.and.arrow.up.left") {
                MarkdownText(summary).font(.callout)
            }
        case .modelChanged(_, let model):
            Label { Text("Model changed to **\(model)**") } icon: { Image(systemName: "cpu") }
                .font(.footnote).foregroundStyle(Palette.text2)
        case let .notice(_, level, message):
            Label(message, systemImage: level == .info ? "info.circle" : "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(level == .error ? Palette.red : level == .warning ? Palette.amber : Palette.text2)
        case .turnEnd(let summary):
            TurnFooter(summary: summary)
        }
    }
}

private struct UserBubble: View {
    let message: UserMessage

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("You").font(.caption.weight(.semibold)).foregroundStyle(Palette.text2)
                Spacer()
                if let at = message.at {
                    Text(at, style: .time).font(.caption).foregroundStyle(Palette.text3)
                }
            }
            if !pictures.isEmpty {
                ScrollView(.horizontal) {
                    HStack { ForEach(Array(pictures.enumerated()), id: \.offset) { _, picture in Thumbnail(image: picture.image, name: picture.name) } }
                }
            }
            if !message.text.isEmpty {
                Text(message.text).textSelection(.enabled)
            }
            if !message.attachments.isEmpty {
                FlowChips(names: message.attachments.map { Format.baseName($0.path) })
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
        .accessibilityElement(children: pictures.isEmpty ? .combine : .contain)
    }

    /// The prompt's images, then those of attached image files.
    private var pictures: [(image: ImageBlock, name: String)] {
        let sent = message.images.enumerated().map { index, image in
            (image, message.images.count == 1 ? "Image" : "Image \(index + 1)")
        }
        let attached = message.attachments.flatMap { attachment in
            attachment.images.map { ($0, Format.baseName(attachment.path)) }
        }
        return sent + attached
    }
}

/// A collapsed section with a title, like the web UI's `<details>`.
struct Disclosure<Content: View>: View {
    let title: String
    let icon: String
    var preview: String?
    @ViewBuilder let content: () -> Content
    @State private var open = false

    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            content().frame(maxWidth: .infinity, alignment: .leading).padding(.top, 6)
        } label: {
            HStack(spacing: 6) {
                Image(systemName: icon).accessibilityHidden(true)
                Text(title)
                if let preview, !open {
                    Text(preview).foregroundStyle(Palette.text3).lineLimit(1)
                }
            }
            .font(.footnote)
            .foregroundStyle(Palette.text2)
        }
        .tint(Palette.text3)
    }
}

/// A small image that opens in the full-screen viewer.
struct Thumbnail: View {
    let image: ImageBlock
    var name = "Image"
    @State private var viewing: ViewerImage?

    var body: some View {
        if let data = Data(base64Encoded: image.data), let picture = UIImage(data: data) {
            Button {
                viewing = ViewerImage(image: picture, data: data, name: name, mediaType: image.mediaType)
            } label: {
                Image(uiImage: picture).resizable().scaledToFill().frame(width: 72, height: 72)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                    .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(Palette.border))
            }
            .buttonStyle(.plain)
            .frame(minWidth: 44, minHeight: 44)
            .accessibilityLabel(name)
            .accessibilityHint("Opens full screen")
            .imageViewer($viewing)
        }
    }
}

private struct FlowChips: View {
    let names: [String]

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(names, id: \.self) { name in
                    Label(name, systemImage: "doc").font(.caption)
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .card(Palette.surface)
                }
            }
        }
        .accessibilityLabel("Attached: \(names.joined(separator: ", "))")
    }
}

private struct PlanCard: View {
    let plan: Plan
    @State private var open: Bool?

    var body: some View {
        DisclosureGroup(isExpanded: Binding(get: { open ?? !plan.superseded }, set: { open = $0 })) {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(plan.items.enumerated()), id: \.offset) { _, todo in
                    Label {
                        Text(todo.content)
                            .strikethrough(todo.status == .completed, color: Palette.text3)
                            .foregroundStyle(todo.status == .completed ? Palette.text3 : Palette.text)
                    } icon: {
                        Image(systemName: todo.status == .completed ? "checkmark.circle.fill" : todo.status == .inProgress ? "circle.dotted" : "circle")
                            .foregroundStyle(todo.status == .inProgress ? Palette.green : Palette.text3)
                    }
                    .font(.callout)
                    .accessibilityValue(todo.status == .completed ? "done" : todo.status == .inProgress ? "in progress" : "pending")
                }
                if let error = plan.error {
                    Text(error).font(.footnote).foregroundStyle(Palette.red)
                }
            }
            .padding(.top, 6)
        } label: {
            HStack {
                Label("Plan", systemImage: "checklist").font(.footnote.weight(.semibold))
                ProgressView(value: Double(plan.completed), total: Double(max(plan.items.count, 1)))
                    .tint(Palette.green)
                    .frame(maxWidth: 80)
                Text("\(plan.completed)/\(plan.items.count)").font(.caption.monospacedDigit())
            }
            .foregroundStyle(Palette.text2)
        }
        .tint(Palette.text3)
        .padding(10)
        .card(Palette.surface)
        .opacity(plan.superseded ? 0.7 : 1)
    }
}

private struct TurnFooter: View {
    let summary: TurnSummary

    var body: some View {
        Text(parts.joined(separator: " · "))
            .font(.caption.monospacedDigit())
            .foregroundStyle(summary.status == .completed ? Palette.text3 : summary.status == .failed ? Palette.red : Palette.amber)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityLabel("Turn ended. " + parts.joined(separator: ", "))
    }

    private var parts: [String] {
        var parts: [String] = []
        switch summary.status {
        case .completed: break
        case .cancelled: parts.append("Stopped")
        case .failed: parts.append("Failed")
        }
        if let ms = summary.milliseconds { parts.append(Format.duration(milliseconds: ms)) }
        if let usage = summary.usage {
            parts.append(Format.usage(usage))
            let cost = Format.cost(usage.cost)
            if !cost.isEmpty { parts.append(cost) }
        }
        if summary.contextTokens > 0 { parts.append("\(Format.tokens(summary.contextTokens)) context") }
        return parts
    }
}

/// What the agent is writing right now.
struct DraftView: View {
    let draft: Draft

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if !draft.thinking.isEmpty {
                Disclosure(title: "Thinking", icon: "lightbulb", preview: Format.truncate(Format.firstLine(String(draft.thinking.suffix(160))), 80)) {
                    Text(draft.thinking).font(.callout).foregroundStyle(Palette.text2)
                }
            }
            if !draft.text.isEmpty {
                MarkdownText(draft.text)
            }
            if let tool = draft.preparingTool {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Preparing \(ToolPresentation.title(tool).title.lowercased())").font(.footnote).foregroundStyle(Palette.text2)
                }
            } else if draft.text.isEmpty {
                ProgressView().controlSize(.small).accessibilityLabel("Thinking")
            }
        }
    }
}
