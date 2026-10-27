# Voice, and files malac sends you

malac can send you files and screenshots, type what you say, read things aloud, and hold a spoken conversation. It all works in every app: the terminal, the web UI, the editor extension, Android and iOS. Each voice feature appears once a model that can do it is available, and each can be turned off.

## Files and screenshots

Ask for something to see ("send me a screenshot of the settings page", "share the generated report") and malac sends it with its `send_to_user` tool. Images show in the conversation; tap or click one to view it full screen, where you can zoom in, pan, and save or share it. Other files show as a card you can open or download. With the control service connected, malac can screenshot your desktop or its browser and send that; without it, it captures the screen with a command and sends the file.

Sent files are copies, kept in `~/.malac/shared/` for 30 days, so they still open after the original changes. Paired devices fetch them through the daemon, like everything else.

## The three voice features

| Feature | What it does | Needs | On by default |
|---|---|---|---|
| Dictation | A microphone button in the composer: speak, and the text appears for you to edit and send | A transcription model (`whisper-1`, `gpt-4o-mini-transcribe`), or a chat model that takes audio (`gpt-4o-audio-preview`) | Yes |
| Speech | malac speaks when it needs you: approvals, questions, and when a long task finishes or something notable happens | A speech model (`gpt-4o-mini-tts`, `tts-1`), or your computer's own voice (`say` on macOS, `espeak-ng` on Linux) | No |
| Talk mode | A spoken conversation: you talk, a voice model hands the work to malac, and tells you what malac did and what it needs | A Realtime model (`gpt-realtime`) | Yes |

malac finds these models in your providers' model lists. Only OpenAI-style providers (OpenAI itself, and gateways such as LiteLLM in front of OpenAI, Groq or a local server) have audio APIs. A gateway may name a model differently on its Realtime route: LiteLLM, for one, can take `gpt-4o-mini-transcribe` for transcription but want `openai/gpt-realtime` for talk mode, so if talk mode says the model is unknown, pick the provider-prefixed name. A model whose name does not say what it does can be declared:

```toml
[models."lora/listener"]
audio = "transcription"   # or "input", "speech", "realtime"
```

## Settings

Every app has a Voice section in its settings: a switch per feature, which model it uses, the voice, and the dictation language. They all write the same `[voice]` table in `~/.malac/config.toml`:

```toml
[voice]
dictation = true
dictation_model = "openai/gpt-4o-mini-transcribe"   # unset: the first model that can
language = "en"                                     # unset: the model works it out
speech = true
speech_model = "openai/gpt-4o-mini-tts"             # or "system"
speech_voice = "coral"
talk = true
talk_model = "openai/gpt-realtime"
talk_voice = "marin"
```

`malac voice` shows what is available and in use; `malac voice set speech true` changes a setting from the terminal.

## Where speech plays

Speech plays on one device: of those with "Speak on this device" on, the one you used last. Answer an approval on your phone and the next sentence plays there; go back to the laptop and it follows you. When speech is on, malac's agents also get a `say` tool, which they use sparingly: when they need you, when a long task is done, or when something is worth knowing. Approval requests are announced for them. After a turn of 30 seconds or more in which the agent said nothing, you hear "Done." and the first sentence of its reply.

## Talk mode

Start talk mode with the headset button (or `/talk` in the terminal) and speak. The voice model knows the project you are in, passes what you ask to malac, says what it asked, and when malac finishes tells you the outcome in a sentence or two. Ask how it is going, and answer approvals out loud ("yes, run it"). Your requests show in the conversation like typed ones, so you can read the details later. Talking over the voice stops it. While talk mode runs, the voice model speaks for malac, so the separate speech above pauses. An approval you answer out loud is passed on by the voice model, so say no when in doubt and check the request on screen.

The phone apps and browsers cancel the speaker's echo. In the terminal, use headphones, or malac pauses the microphone while it speaks.

## In the apps

- **Web UI:** the microphone and headset buttons sit in the composer. Ctrl+Shift+Space (⌘⇧Space on a Mac) starts and stops dictation; Enter puts the text in the message box and Esc throws the recording away. A speaker button in the header switches "Speak on this device"; Settings has the Voice section.
- **VS Code and Cursor:** editors keep the microphone from their webviews, so the extension runs the malac CLI below for the microphone and speaker. The composer's microphone button or **malac: Dictate** (⌥⇧⌘D, Ctrl+Alt+Shift+D elsewhere) records; **malac: Talk** and the headphones button open talk mode in a terminal; **malac: Voice Settings** has the switches, models and voices; `malac.speakOnThisDevice` decides whether the editor speaks. Voice needs malac on the same machine as the editor.
- **Android and iOS:** the microphone in the composer dictates, the headset button opens talk mode, and the Voice section under Daemons has the settings and "Speak on this device". The apps ask for the microphone the first time you use it.

## In the terminal

In the full-screen interface, **Ctrl-X** starts dictation when it is ready: the prompt shows a red dot and the time, Ctrl-X or Enter types what you said at the cursor, and Esc throws it away. `/talk` starts talk mode (its state shows in the status line; Esc ends it), `/speak on|off` decides whether this terminal speaks, `/voice` shows what is available, and `/voice speech true` (any key of the `[voice]` table above) changes one; the View menu has the same switches. Files malac sends show as their names and sizes: click one, press `o` on it in the transcript, or type `/open` (the newest; `/open 2` the one before) to open it, or a screenshot a tool returned, in the system viewer. The line interface (`--plain`) prints each file's name, size and where it is.

| Command | |
|---|---|
| `malac voice [status --json]` | What is available and in use |
| `malac voice set <setting> <value>` | `true`/`false` for `dictation`, `speech`, `talk`; a model or voice otherwise; `none` removes it |
| `malac voice dictate [--json] [--max-seconds N]` | Record until Enter or the end of input (at most N seconds, default 120), print the text; Ctrl-C cancels |
| `malac voice say <text>` | Read text aloud with the speech model |
| `malac voice play <file>` | Play a WAV or MP3 file |
| `malac voice talk [--tab ID] [--agent ID] [--duplex]` | Talk mode without the full-screen interface: Esc ends, `m` mutes, space stops the voice; `--duplex` keeps the microphone open for headphones |

Each takes `-C <name>` to use a saved connection's daemon with this machine's microphone and speaker. On macOS malac uses Core Audio, and the first recording asks whether your terminal (or editor) may use the microphone; allow it in System Settings > Privacy & Security > Microphone. On Linux it records with `pw-record`, `parecord` or `arecord` and plays with `pw-play`, `paplay` or `aplay` (MP3 with `ffplay`, `mpg123` or `mpv`), so install PipeWire, PulseAudio or ALSA's utilities.

## Privacy

Recordings go to the daemon, which sends them to the model you chose and keeps nothing. Browsers allow the microphone only on secure pages: `https://` (a tunnel) or `localhost`. On a plain `http://` network address the microphone buttons say so.
