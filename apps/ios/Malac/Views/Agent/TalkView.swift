import MalacKit
import SwiftUI

/// Talk mode, full screen: what the voice is doing, what was said, and
/// mute and end. The screen stays on while it is open.
struct TalkView: View {
    let session: DaemonSession
    /// Who the voice works through, e.g. the agent's name.
    let title: String
    @State private var model: TalkModel
    @State private var typed = ""
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @FocusState private var typing: Bool

    init(session: DaemonSession, tab: String?, agent: String?, title: String) {
        self.session = session
        self.title = title
        _model = State(initialValue: TalkModel(endpoint: session.api.endpoint, tab: tab, agent: agent, speech: session.speech))
    }

    var body: some View {
        let state = model.state
        VStack(spacing: 0) {
            header(state)
            Divider()
            transcript(state)
            Divider()
            controls(state)
        }
        .background(Palette.page)
        .task { await model.start() }
        .onAppear { UIApplication.shared.isIdleTimerDisabled = true }
        .onDisappear {
            UIApplication.shared.isIdleTimerDisabled = false
            model.end()
        }
        .onChange(of: scenePhase) { _, phase in
            // Without background audio the microphone stops when the app leaves the screen.
            if phase == .background { model.end("Talk mode ended when malac left the screen.") }
        }
    }

    // MARK: Header

    private func header(_ state: TalkState) -> some View {
        VStack(spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Talk mode").font(.headline)
                    Text(subtitle(state)).font(.footnote).foregroundStyle(Palette.text2).lineLimit(1)
                }
                Spacer()
            }
            ZStack {
                Circle().fill(Palette.surface)
                Circle().strokeBorder(state.phase == .listening && !model.muted ? Palette.green : Palette.border)
                if state.phase == .connecting {
                    ProgressView()
                } else {
                    Image(systemName: symbol(state.phase))
                        .font(.system(size: 30, weight: .regular))
                        .foregroundStyle(Palette.text)
                }
            }
            .frame(width: 96, height: 96)
            .accessibilityHidden(true)
            Text(model.muted && state.phase == .listening ? "Muted" : state.phase.label)
                .font(.title3.weight(.semibold))
                .foregroundStyle(model.muted && state.phase == .listening ? Palette.amber : Palette.text)
                .accessibilityLabel("Talk mode: \(model.muted && state.phase == .listening ? "muted" : state.phase.label)")
            LevelMeter(levels: model.levels.values, height: 20)
                .opacity(state.phase == .listening && !model.muted ? 1 : 0.3)
            Text(hint(state))
                .font(.footnote)
                .foregroundStyle(Palette.text2)
                .multilineTextAlignment(.center)
                .frame(minHeight: 36, alignment: .top)
        }
        .padding(16)
    }

    private func subtitle(_ state: TalkState) -> String {
        var parts = [title]
        if let voice = state.voice { parts.append(voice) }
        return parts.joined(separator: " · ")
    }

    private func symbol(_ phase: TalkState.Phase) -> String {
        switch phase {
        case .connecting: "antenna.radiowaves.left.and.right"
        case .listening: model.muted ? "mic.slash" : "mic"
        case .thinking: "ellipsis"
        case .speaking: "speaker.wave.2"
        case .ended: "stop"
        }
    }

    private func hint(_ state: TalkState) -> String {
        switch state.phase {
        case .connecting: "Connecting to the voice model."
        case .listening: model.muted ? "Unmute to talk." : "Speak. The voice passes what you ask to malac."
        case .thinking: "Working out what you said."
        case .speaking: "Talk over the voice to interrupt it."
        case .ended: state.endReason ?? ""
        }
    }

    // MARK: Transcript

    private func transcript(_ state: TalkState) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    ForEach(state.lines) { line in
                        LineView(line: line).id(line.id)
                    }
                    if state.lines.isEmpty, state.phase != .ended {
                        Text("What you and the voice say shows here.")
                            .font(.callout)
                            .foregroundStyle(Palette.text3)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 24)
                    }
                }
                .padding(16)
            }
            .defaultScrollAnchor(.bottom)
            .onChange(of: state.lines.last?.text) {
                if let last = state.lines.last { proxy.scrollTo(last.id, anchor: .bottom) }
            }
        }
    }

    // MARK: Controls

    @ViewBuilder
    private func controls(_ state: TalkState) -> some View {
        VStack(spacing: 10) {
            if state.phase == .ended {
                HStack(spacing: 10) {
                    if model.needsPermission {
                        Button("Open Settings") { Microphone.openSettings() }
                            .buttonStyle(.bordered)
                            .tint(Palette.text2)
                    }
                    Button { dismiss() } label: {
                        Text("Close").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(Palette.primary)
                    .foregroundStyle(Palette.onPrimary)
                    .keyboardShortcut(.cancelAction)
                }
            } else {
                if state.ready {
                    HStack(spacing: 8) {
                        TextField("Type instead", text: $typed)
                            .focused($typing)
                            .submitLabel(.send)
                            .onSubmit(sendTyped)
                            .padding(.horizontal, 10)
                            .frame(minHeight: 44)
                            .card(Palette.surface)
                        Button(action: sendTyped) {
                            Image(systemName: "arrow.up.circle.fill").font(.title2).frame(minWidth: 44, minHeight: 44)
                        }
                        .foregroundStyle(typed.trimmingCharacters(in: .whitespaces).isEmpty ? Palette.text3 : Palette.primary)
                        .disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)
                        .accessibilityLabel("Send")
                    }
                }
                HStack(spacing: 10) {
                    Button { model.toggleMute() } label: {
                        Label(model.muted ? "Unmute" : "Mute", systemImage: model.muted ? "mic" : "mic.slash")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                    .tint(Palette.text)
                    .disabled(!state.ready)
                    if state.phase == .speaking {
                        Button { model.interrupt() } label: {
                            Label("Interrupt", systemImage: "hand.raised").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                        .tint(Palette.text)
                    }
                    Button {
                        model.end()
                        dismiss()
                    } label: {
                        Label("End", systemImage: "xmark").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(Palette.primary)
                    .foregroundStyle(Palette.onPrimary)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityLabel("End talk mode")
                }
            }
        }
        .controlSize(.large)
        .padding(16)
        .background(Palette.page)
    }

    private func sendTyped() {
        model.send(typed)
        typed = ""
    }
}

private struct LineView: View {
    let line: TalkState.Line

    var body: some View {
        switch line.kind {
        case .user, .voice:
            VStack(alignment: .leading, spacing: 4) {
                Text(line.kind == .user ? "You" : "malac")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Palette.text2)
                Text(line.text)
                    .foregroundStyle(line.final ? Palette.text : Palette.text2)
                    .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
        case .tool:
            Label(line.text, systemImage: "arrow.turn.down.right")
                .font(.footnote)
                .foregroundStyle(Palette.text2)
        case .error:
            Label(line.text, systemImage: "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(Palette.red)
        }
    }
}
