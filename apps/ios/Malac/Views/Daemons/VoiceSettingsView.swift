import MalacKit
import SwiftUI

/// The daemon's `[voice]` settings, shared by every app: dictation, speech
/// and talk mode, their models and voices, plus whether this device speaks.
struct VoiceSettingsView: View {
    let session: DaemonSession
    @AppStorage(DaemonSession.speakHereKey) private var speakHere = true
    @State private var language = ""
    @State private var refreshing = false

    var body: some View {
        List {
            if let voice = session.state.voice {
                sections(voice)
            } else {
                Text("This daemon has no voice features. Update malac on the computer to dictate, hear and talk.")
                    .font(.callout)
                    .foregroundStyle(Palette.text2)
                    .listRowBackground(Palette.surface)
            }
        }
        .malacList()
        .navigationTitle("Voice")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { language = session.state.voice?.language ?? "" }
        .onChange(of: session.state.voice?.language) { _, value in language = value ?? "" }
        .onChange(of: speakHere) { session.syncListening() }
        .refreshable { await session.refreshVoice() }
    }

    @ViewBuilder
    private func sections(_ voice: VoiceStatus) -> some View {
        Section {
            Toggle("Dictation", isOn: toggle(voice.dictation.enabled) { .dictation($0) })
            ModelPicker(title: "Model", feature: voice.dictation) { await session.setVoice(.dictationModel($0)) }
            LabeledContent("Language") {
                TextField("Automatic", text: $language)
                    .multilineTextAlignment(.trailing)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .submitLabel(.done)
                    .onSubmit { Task { await saveLanguage() } }
            }
        } header: {
            Text("Dictation")
        } footer: {
            Text(voice.dictation.models.isEmpty
                ? "No model can transcribe. Add an OpenAI-style provider with a transcription model such as whisper-1 or gpt-4o-mini-transcribe."
                : "A microphone button in the composer turns what you say into text. The language is a code such as en or de; leave it empty and the model works it out.")
        }
        .listRowBackground(Palette.surface)

        Section {
            Toggle("Speech", isOn: toggle(voice.speech.enabled) { .speech($0) })
            ModelPicker(title: "Model", feature: voice.speech) { await session.setVoice(.speechModel($0)) }
            // The computer's own voice speaks in the system's voice.
            if voice.speech.model != "system" {
                VoicePicker(current: voice.speech.voice, voices: voice.voices) { await session.setVoice(.speechVoice($0)) }
            }
            Toggle("Speak on this device", isOn: $speakHere)
        } header: {
            Text("Speech")
        } footer: {
            Text(voice.speech.models.isEmpty
                ? "No model can speak. Add a speech model such as tts-1, or use the computer's own voice."
                : "malac reads approvals, questions and notable results aloud, on the device you used last among those that speak.")
        }
        .listRowBackground(Palette.surface)

        Section {
            Toggle("Talk mode", isOn: toggle(voice.talk.enabled) { .talk($0) })
            ModelPicker(title: "Model", feature: voice.talk) { await session.setVoice(.talkModel($0)) }
            VoicePicker(current: voice.talk.voice, voices: voice.voices) { await session.setVoice(.talkVoice($0)) }
        } header: {
            Text("Talk mode")
        } footer: {
            Text(voice.talk.models.isEmpty
                ? "No model can talk. Add a Realtime model such as gpt-realtime."
                : "The headphones button in an agent or tab starts a spoken conversation: a voice model passes what you say to malac and tells you what it did.")
        }
        .listRowBackground(Palette.surface)

        Section {
            Button {
                Task {
                    refreshing = true
                    await session.refreshVoice()
                    refreshing = false
                }
            } label: {
                HStack {
                    Text("Check for models")
                    Spacer()
                    if refreshing { ProgressView() }
                }
            }
            .disabled(refreshing || !session.isOnline)
            if let checked = voice.checked {
                LabeledContent("Last checked") {
                    Text(checked, style: .relative) + Text(" ago")
                }
            }
        } footer: {
            Text("Reads the providers' model lists again, for models added since the daemon started.")
        }
        .listRowBackground(Palette.surface)
    }

    private func toggle(_ value: Bool, _ change: @escaping (Bool) -> VoiceSetting) -> Binding<Bool> {
        Binding(get: { value }, set: { on in Task { await session.setVoice(change(on)) } })
    }

    private func saveLanguage() async {
        let value = language.trimmingCharacters(in: .whitespaces)
        guard value != (session.state.voice?.language ?? "") else { return }
        await session.setVoice(.language(value.isEmpty ? nil : value))
    }
}

/// The model a feature uses: automatic (the first that can) or one of them.
private struct ModelPicker: View {
    let title: String
    let feature: VoiceFeature
    let change: (String?) async -> Void

    var body: some View {
        Picker(title, selection: Binding(get: { feature.configured ?? "" }, set: { model in
            Task { await change(model.isEmpty ? nil : model) }
        })) {
            Text(feature.models.first.map { "Automatic (\($0))" } ?? "Automatic").tag("")
            ForEach(feature.models, id: \.self) { Text($0).tag($0) }
            if let configured = feature.configured, !feature.models.contains(configured) {
                Text("\(configured) (not available)").tag(configured)
            }
        }
        .disabled(feature.models.isEmpty && feature.configured == nil)
    }
}

/// The voice of speech or talk mode, from those the models know.
private struct VoicePicker: View {
    let current: String?
    let voices: [String]
    let change: (String?) async -> Void

    var body: some View {
        if !voices.isEmpty || current != nil {
            Picker("Voice", selection: Binding(get: { current ?? "" }, set: { voice in
                Task { await change(voice.isEmpty ? nil : voice) }
            })) {
                if current == nil { Text("Default").tag("") }
                ForEach(voices, id: \.self) { Text($0).tag($0) }
                if let current, !voices.contains(current) {
                    Text(current).tag(current)
                }
            }
        }
    }
}
