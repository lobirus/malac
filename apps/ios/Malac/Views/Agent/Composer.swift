import MalacKit
import PhotosUI
import SwiftUI

/// Sends a prompt to an agent, or starts a new one in a tab. Picks the model
/// and the tab's mode, attaches photos and screenshots, dictates when the
/// daemon has a dictation model, and stops a turn.
struct Composer: View {
    let session: DaemonSession
    let agent: AgentState?
    let tab: WorkspaceTab?
    var onSend: ([ImageBlock], String) -> Void = { _, _ in }
    var onStarted: (String) -> Void = { _ in }

    @State private var text = ""
    @State private var picked: [PhotosPickerItem] = []
    @State private var images: [ImageAttachment] = []
    @State private var newModel: String?
    @State private var sending = false
    @State private var viewing: ViewerImage?
    @State private var dictation = Dictation()
    @FocusState private var focused: Bool
    @Environment(\.scenePhase) private var scenePhase

    private static let maxImages = 6

    var body: some View {
        let running = agent.map { session.state.phase(of: $0) == .running || session.state.phase(of: $0) == .awaitingApproval } ?? false
        VStack(spacing: 8) {
            if !images.isEmpty {
                ScrollView(.horizontal) {
                    HStack(spacing: 8) {
                        ForEach(images) { image in
                            Button {
                                viewing = ViewerImage(image: image.preview, data: Data(base64Encoded: image.input.data),
                                                      name: image.input.name ?? "Image", mediaType: image.input.mediaType)
                            } label: {
                                Image(uiImage: image.preview)
                                    .resizable().scaledToFill()
                                    .frame(width: 56, height: 56)
                                    .clipShape(RoundedRectangle(cornerRadius: 4))
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(image.input.name ?? "Image")
                            .accessibilityHint("Opens full screen")
                            .overlay(alignment: .topTrailing) {
                                Button {
                                    images.removeAll { $0.id == image.id }
                                } label: {
                                    Image(systemName: "xmark.circle.fill").symbolRenderingMode(.palette)
                                        .foregroundStyle(Palette.onPrimary, Palette.primary)
                                }
                                .offset(x: 6, y: -6)
                                .accessibilityLabel("Remove image")
                            }
                        }
                    }
                    .padding(.top, 6)
                    .padding(.horizontal, 2)
                }
            }
            if dictation.phase != .idle {
                DictationBar(dictation: dictation)
            } else {
                inputRow(running: running)
            }
            HStack(spacing: 12) {
                modelMenu
                if let tab {
                    ModeMenu(mode: tab.mode) { session.setMode($0, tab: tab.id) }
                        .labelStyle(.titleAndIcon)
                }
                Spacer()
            }
            .font(.caption)
            .foregroundStyle(Palette.text2)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Palette.page)
        .overlay(alignment: .top) { Divider() }
        .onChange(of: picked) { _, items in
            Task { await load(items) }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .background { dictation.finish() }
        }
        .onDisappear { dictation.cancel() }
        .imageViewer($viewing)
    }

    private func inputRow(running: Bool) -> some View {
        HStack(alignment: .bottom, spacing: 10) {
            PhotosPicker(selection: $picked, maxSelectionCount: Self.maxImages, matching: .images) {
                Image(systemName: "photo.badge.plus").font(.title3).frame(minWidth: 36, minHeight: 36)
            }
            .foregroundStyle(Palette.text2)
            .accessibilityLabel("Attach photos")
            TextField(agent == nil ? "What should the new agent do?" : "Message \(agent?.label ?? "the agent")", text: $text, axis: .vertical)
                .lineLimit(1 ... 6)
                .focused($focused)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .card(Palette.surface)
            if session.state.voice?.dictation.isReady == true {
                Button {
                    Task { await dictate() }
                } label: {
                    Image(systemName: "mic").font(.title3).frame(minWidth: 36, minHeight: 36)
                }
                .foregroundStyle(Palette.text2)
                .disabled(!session.isOnline)
                .accessibilityLabel("Dictate")
                .accessibilityHint("Records what you say and adds it to the message")
            }
            if running, let agent {
                Button {
                    session.cancel(agent.id)
                } label: {
                    Image(systemName: "stop.circle.fill").font(.title2).frame(minWidth: 36, minHeight: 36)
                }
                .foregroundStyle(Palette.red)
                .accessibilityLabel("Stop")
            }
            Button {
                Task { await send() }
            } label: {
                if sending {
                    ProgressView().frame(minWidth: 36, minHeight: 36)
                } else {
                    Image(systemName: "arrow.up.circle.fill").font(.title2).frame(minWidth: 36, minHeight: 36)
                }
            }
            .foregroundStyle(canSend ? Palette.primary : Palette.text3)
            .disabled(!canSend)
            .accessibilityLabel(agent == nil ? "Start agent" : "Send")
        }
    }

    private var canSend: Bool {
        !sending && session.isOnline && (!text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty)
            && (agent != nil || tab != nil)
    }

    private var models: [String] { session.state.models }

    @ViewBuilder
    private var modelMenu: some View {
        if !models.isEmpty {
            let current = agent?.model ?? newModel ?? tab?.options?.model ?? session.state.defaultModel ?? ""
            Menu {
                // A section per provider, so models of another gateway are easy to find.
                Picker("Model", selection: Binding(get: { current }, set: { model in
                    if let agent { session.setModel(model, agent: agent.id) } else { newModel = model }
                })) {
                    ForEach(session.state.modelGroups) { group in
                        if group.provider.isEmpty {
                            ForEach(group.models, id: \.self) { Text($0).tag($0) }
                        } else {
                            Section(group.provider) {
                                ForEach(group.models, id: \.self) { Text(ModelGroup.name(of: $0)).tag($0) }
                            }
                        }
                    }
                }
            } label: {
                Label(current.isEmpty ? "Model" : current, systemImage: "cpu").lineLimit(1)
            }
            .accessibilityLabel("Model, \(current)")
        }
    }

    private func dictate() async {
        dictation.onText = { heard in
            let separator = text.isEmpty || text.last?.isWhitespace == true ? "" : " "
            text += separator + heard
            focused = true
        }
        dictation.onError = { message in
            session.notice = DaemonSession.Notice(level: .error, text: message)
        }
        await dictation.start(api: session.api, speech: session.speech)
    }

    private func load(_ items: [PhotosPickerItem]) async {
        guard !items.isEmpty else { return }
        for (index, item) in items.enumerated() where images.count < Self.maxImages {
            guard let data = try? await item.loadTransferable(type: Data.self),
                  let image = ImageAttachment(data: data, name: "image-\(images.count + index + 1).jpg") else { continue }
            images.append(image)
        }
        picked = []
    }

    private func send() async {
        let prompt = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let inputs = images.map(\.input)
        sending = true
        defer { sending = false }
        onSend(inputs.map(\.block), prompt)
        do {
            let started = try await session.prompt(prompt, images: inputs, agent: agent?.id, tab: tab?.id, model: newModel)
            text = ""
            images = []
            focused = false
            if agent == nil, let started { onStarted(started) }
        } catch {
            session.notice = DaemonSession.Notice(level: .error, text: error.localizedDescription)
        }
    }
}

/// Recording or transcribing, in place of the text field.
private struct DictationBar: View {
    let dictation: Dictation

    var body: some View {
        HStack(spacing: 10) {
            Button {
                dictation.cancel()
            } label: {
                Image(systemName: "xmark").font(.body.weight(.medium)).frame(minWidth: 36, minHeight: 36)
            }
            .foregroundStyle(Palette.text2)
            .accessibilityLabel("Cancel dictation")
            switch dictation.phase {
            case .recording(let since):
                Circle().fill(Palette.red).frame(width: 8, height: 8).accessibilityHidden(true)
                Text(since, style: .timer)
                    .font(.callout.monospacedDigit())
                    .accessibilityLabel("Recording")
                    .accessibilityValue(Text(since, style: .timer))
                LevelMeter(levels: dictation.levels.values)
                Spacer(minLength: 4)
                Button {
                    dictation.finish()
                } label: {
                    Image(systemName: "checkmark.circle.fill").font(.title2).frame(minWidth: 36, minHeight: 36)
                }
                .foregroundStyle(Palette.primary)
                .accessibilityLabel("Stop and transcribe")
            case .transcribing:
                ProgressView().controlSize(.small)
                Text("Transcribing…").font(.callout).foregroundStyle(Palette.text2)
                Spacer(minLength: 4)
            case .idle:
                EmptyView()
            }
        }
        .padding(.horizontal, 6)
        .frame(minHeight: 44)
        .card(Palette.surface)
    }
}
