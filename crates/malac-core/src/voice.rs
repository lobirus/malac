//! Speech: which models can transcribe, speak or hold a spoken
//! conversation, and the calls that use them. Only OpenAI-style providers
//! have sound APIs; Anthropic's do not.

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{self, AudioKind, Config, ProviderConfig, ProviderKind};
use crate::provider::http_client;

/// `speech_model` for the operating system's own voice.
pub const SYSTEM: &str = "system";
/// Voices of OpenAI's speech and Realtime models; `alloy` works with all of them.
pub const VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
    "marin", "cedar",
];
pub const DEFAULT_VOICE: &str = "alloy";
/// What `/audio/transcriptions` accepts at most.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;
/// Longest text read aloud at once.
pub const MAX_SPEECH_CHARS: usize = 4096;

/// What a model does with sound: `[models."p/m"] audio` when set, else its name.
pub fn audio_kind(model: &str, config: Option<AudioKind>) -> Option<AudioKind> {
    if config.is_some() {
        return config;
    }
    let name = model.to_ascii_lowercase();
    let has = |parts: &[&str]| parts.iter().any(|p| name.contains(p));
    if has(&["moderation", "embed"]) {
        None
    } else if has(&["realtime"]) {
        Some(AudioKind::Realtime)
    } else if has(&[
        "whisper",
        "transcribe",
        "speech-to-text",
        "stt",
        "parakeet",
        "canary",
    ]) {
        Some(AudioKind::Transcription)
    } else if has(&["tts", "text-to-speech", "speech", "kokoro"]) {
        Some(AudioKind::Speech)
    } else if has(&["audio", "voxtral", "omni"]) {
        Some(AudioKind::Input)
    } else {
        None
    }
}

/// The models that can do each voice feature, as `provider/model`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Offers {
    /// Transcription models first, then chat models that take audio.
    pub dictation: Vec<String>,
    /// Speech models, then `system` when this machine has a voice.
    pub speech: Vec<String>,
    pub talk: Vec<String>,
}

/// What `config` and the providers' model lists (`available`, as
/// `(provider, models)`) offer.
pub fn offers(config: &Config, available: &[(String, Vec<String>)], system_voice: bool) -> Offers {
    let mut transcription = Vec::new();
    let mut input = Vec::new();
    let mut offers = Offers::default();
    for (name, provider) in &config.providers {
        if provider.kind != ProviderKind::OpenAi {
            continue;
        }
        let listed = available
            .iter()
            .filter(|(p, _)| p == name)
            .flat_map(|(_, models)| models.iter());
        let mut seen: Vec<&String> = Vec::new();
        for model in provider.models.iter().chain(listed) {
            if seen.contains(&model) {
                continue;
            }
            seen.push(model);
            let spec = format!("{name}/{model}");
            let list = match audio_kind(model, config.model_config(&spec).audio) {
                Some(AudioKind::Transcription) => &mut transcription,
                Some(AudioKind::Input) => &mut input,
                Some(AudioKind::Speech) => &mut offers.speech,
                Some(AudioKind::Realtime) => &mut offers.talk,
                None => continue,
            };
            list.push(spec);
        }
    }
    offers.dictation = transcription.into_iter().chain(input).collect();
    if system_voice {
        offers.speech.push(SYSTEM.to_owned());
    }
    offers
}

/// The model a feature uses: the configured one, else the first offered.
pub fn choose(configured: Option<&str>, offered: &[String]) -> Option<String> {
    configured
        .filter(|m| !m.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| offered.first().cloned())
}

/// The provider behind `provider/model`, its key and the model id.
pub struct Endpoint<'a> {
    pub provider: &'a ProviderConfig,
    pub key: Option<String>,
    pub model: &'a str,
    pub kind: Option<AudioKind>,
}

pub fn endpoint<'a>(config: &'a Config, spec: &'a str) -> Result<Endpoint<'a>> {
    let (name, model) = spec
        .split_once('/')
        .with_context(|| format!("`{spec}` is not `provider/model`"))?;
    let provider = config
        .providers
        .get(name)
        .with_context(|| format!("no provider `{name}`"))?;
    if provider.kind != ProviderKind::OpenAi {
        bail!("`{name}` is an Anthropic provider, which has no audio API");
    }
    Ok(Endpoint {
        provider,
        key: config::api_key(name, provider),
        model,
        kind: audio_kind(model, config.model_config(spec).audio),
    })
}

impl Endpoint<'_> {
    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.provider.base_url.trim_end_matches('/'))
    }

    fn request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut builder = builder;
        if let Some(key) = &self.key {
            builder = builder.bearer_auth(key);
        }
        for (name, value) in &self.provider.headers {
            builder = builder.header(name, value);
        }
        builder
    }

    /// The Realtime API's WebSocket address for this model.
    pub fn realtime_url(&self) -> String {
        let base = self.provider.base_url.trim_end_matches('/');
        let base = match base.split_once("://") {
            Some(("https", rest)) => format!("wss://{rest}"),
            Some(("http", rest)) => format!("ws://{rest}"),
            _ => base.to_owned(),
        };
        let model: String = url_encode(self.model);
        format!("{base}/realtime?model={model}")
    }

    /// Headers for the Realtime WebSocket: the key and the provider's own.
    pub fn realtime_headers(&self) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> =
            self.provider.headers.clone().into_iter().collect();
        if let Some(key) = &self.key {
            headers.push(("Authorization".into(), format!("Bearer {key}")));
        }
        headers
    }
}

fn url_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Turns recorded speech into text. `media_type` is the recording's, like
/// `audio/wav` or `audio/webm`; chat models that take audio need WAV or MP3.
pub async fn transcribe(
    config: &Config,
    spec: &str,
    audio: Vec<u8>,
    media_type: &str,
    language: Option<&str>,
) -> Result<String> {
    if audio.is_empty() {
        bail!("the recording is empty");
    }
    if audio.len() > MAX_AUDIO_BYTES {
        bail!("the recording is over {} MB", MAX_AUDIO_BYTES / 1024 / 1024);
    }
    let endpoint = endpoint(config, spec)?;
    let text = if endpoint.kind == Some(AudioKind::Input) {
        let format = match media_type {
            "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
            "audio/mpeg" | "audio/mp3" => "mp3",
            other => bail!("{spec} takes WAV or MP3 audio, not {other}"),
        };
        let mut instruction = String::from(
            "Transcribe the speech in this recording exactly, in the language spoken. Reply with the transcript only, no quotes or notes.",
        );
        if let Some(language) = language {
            instruction.push_str(&format!(" The language is probably {language}."));
        }
        let body = json!({
            "model": endpoint.model,
            "messages": [
                { "role": "system", "content": instruction },
                { "role": "user", "content": [
                    { "type": "input_audio", "input_audio": { "data": BASE64.encode(&audio), "format": format } }
                ] }
            ]
        });
        let reply: Value = send(
            endpoint.request(
                http_client()
                    .post(endpoint.url("chat/completions"))
                    .json(&body),
            ),
        )
        .await?
        .json()
        .await?;
        reply["choices"][0]["message"]["content"]
            .as_str()
            .context("the model sent no transcript")?
            .to_owned()
    } else {
        let file = reqwest::multipart::Part::bytes(audio)
            .file_name(format!("speech.{}", extension(media_type)))
            .mime_str(media_type)
            .context("invalid audio type")?;
        let mut form = reqwest::multipart::Form::new()
            .text("model", endpoint.model.to_owned())
            .text("response_format", "json")
            .part("file", file);
        if let Some(language) = language {
            form = form.text("language", language.to_owned());
        }
        let reply: Value = send(
            endpoint.request(
                http_client()
                    .post(endpoint.url("audio/transcriptions"))
                    .multipart(form),
            ),
        )
        .await?
        .json()
        .await?;
        reply["text"]
            .as_str()
            .context("the model sent no transcript")?
            .to_owned()
    };
    Ok(text.trim().to_owned())
}

fn extension(media_type: &str) -> &'static str {
    match media_type.split(';').next().unwrap_or_default().trim() {
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/flac" => "flac",
        _ => "wav",
    }
}

/// Speech for `text`, with its media type: MP3 from a speech model (or
/// what it sends instead), WAV from the system voice.
pub async fn synthesize(
    config: &Config,
    spec: &str,
    voice: Option<&str>,
    text: &str,
) -> Result<(Vec<u8>, &'static str)> {
    let text: String = text.chars().take(MAX_SPEECH_CHARS).collect();
    if text.trim().is_empty() {
        bail!("nothing to say");
    }
    if spec == SYSTEM {
        return Ok((system_speech(voice, &text).await?, "audio/wav"));
    }
    let endpoint = endpoint(config, spec)?;
    let body = json!({
        "model": endpoint.model,
        "input": text,
        "voice": voice.unwrap_or(DEFAULT_VOICE),
        "response_format": "mp3",
    });
    let response =
        send(endpoint.request(http_client().post(endpoint.url("audio/speech")).json(&body)))
            .await?;
    // What arrived, since some servers send WAV whatever was asked for.
    let media_type = match response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim)
    {
        Some("audio/wav" | "audio/x-wav" | "audio/wave") => "audio/wav",
        Some("audio/ogg" | "audio/opus") => "audio/ogg",
        Some("audio/aac") => "audio/aac",
        Some("audio/flac") => "audio/flac",
        _ => "audio/mpeg",
    };
    Ok((response.bytes().await?.to_vec(), media_type))
}

/// The command that speaks into a WAV file on this machine, if there is one.
fn system_voice_command() -> Option<&'static str> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["say"]
    } else {
        &["espeak-ng", "espeak"]
    };
    candidates.iter().copied().find(|c| which(c))
}

/// Whether this machine can speak without a model.
pub fn has_system_voice() -> bool {
    system_voice_command().is_some()
}

fn which(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(command)))
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

async fn system_speech(voice: Option<&str>, text: &str) -> Result<Vec<u8>> {
    let command =
        system_voice_command().context("this machine has no system voice (install espeak-ng)")?;
    let dir = tempfile_dir()?;
    let out = dir.join("speech.wav");
    let mut cmd = tokio::process::Command::new(command);
    if command == "say" {
        cmd.args(["--file-format=WAVE", "--data-format=LEI16@22050", "-o"])
            .arg(&out);
    } else {
        cmd.arg("-w").arg(&out);
    }
    if let Some(voice) = voice.filter(|v| !v.is_empty()) {
        cmd.args(["-v", voice]);
    }
    // The text goes in on stdin so that a leading `-` is not read as a flag.
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("cannot run {command}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(text.as_bytes()).await?;
    }
    let output = child.wait_with_output().await?;
    let result = if output.status.success() {
        std::fs::read(&out).with_context(|| format!("{command} wrote no audio"))
    } else {
        Err(anyhow::anyhow!(
            "{command} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("malac-voice-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let response = request.send().await.context("cannot reach the provider")?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or(body);
    bail!(
        "{status}: {}",
        message.chars().take(400).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, ProviderConfig};

    #[test]
    fn tells_models_apart_by_name() {
        let kind = |m| audio_kind(m, None);
        assert_eq!(kind("whisper-1"), Some(AudioKind::Transcription));
        assert_eq!(
            kind("gpt-4o-mini-transcribe"),
            Some(AudioKind::Transcription)
        );
        assert_eq!(
            kind("groq/whisper-large-v3"),
            Some(AudioKind::Transcription)
        );
        assert_eq!(kind("gpt-4o-mini-tts"), Some(AudioKind::Speech));
        assert_eq!(kind("tts-1-hd"), Some(AudioKind::Speech));
        assert_eq!(kind("gpt-realtime"), Some(AudioKind::Realtime));
        assert_eq!(
            kind("gpt-4o-mini-realtime-preview"),
            Some(AudioKind::Realtime)
        );
        assert_eq!(kind("gpt-4o-audio-preview"), Some(AudioKind::Input));
        assert_eq!(kind("voxtral-small"), Some(AudioKind::Input));
        assert_eq!(kind("gpt-5"), None);
        assert_eq!(kind("smart"), None);
        assert_eq!(kind("omni-moderation-latest"), None);
        assert_eq!(
            audio_kind("smart", Some(AudioKind::Input)),
            Some(AudioKind::Input)
        );
    }

    #[test]
    fn offers_what_openai_style_providers_have() {
        let mut config = Config::default();
        config.providers.insert(
            "lora".into(),
            ProviderConfig {
                models: vec!["smart".into(), "whisper-1".into()],
                ..Default::default()
            },
        );
        config.providers.insert(
            "claude".into(),
            ProviderConfig {
                kind: ProviderKind::Anthropic,
                models: vec!["whisper-lookalike".into()],
                ..Default::default()
            },
        );
        config.models.insert(
            "lora/gemini".into(),
            ModelConfig {
                audio: Some(AudioKind::Input),
                ..Default::default()
            },
        );
        let available = vec![(
            "lora".to_owned(),
            vec![
                "whisper-1".into(),
                "gemini".into(),
                "gpt-4o-mini-tts".into(),
                "gpt-realtime".into(),
            ],
        )];
        let offers = offers(&config, &available, true);
        assert_eq!(offers.dictation, ["lora/whisper-1", "lora/gemini"]);
        assert_eq!(offers.speech, ["lora/gpt-4o-mini-tts", "system"]);
        assert_eq!(offers.talk, ["lora/gpt-realtime"]);
        assert_eq!(
            choose(None, &offers.talk).as_deref(),
            Some("lora/gpt-realtime")
        );
        assert_eq!(
            choose(Some("lora/x"), &offers.talk).as_deref(),
            Some("lora/x")
        );
        assert_eq!(choose(None, &[]), None);
    }

    #[test]
    fn realtime_addresses_use_websockets() {
        let mut config = Config::default();
        config.providers.insert(
            "lora".into(),
            ProviderConfig {
                base_url: "https://gateway.example.com/v1/".into(),
                ..Default::default()
            },
        );
        let realtime = endpoint(&config, "lora/gpt-realtime").unwrap();
        assert_eq!(
            realtime.realtime_url(),
            "wss://gateway.example.com/v1/realtime?model=gpt-realtime"
        );
        assert!(endpoint(&config, "nope").is_err());
    }
}
