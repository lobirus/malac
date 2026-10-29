// Dictation, speech and talk mode. Dictation records the microphone, sends
// 16 kHz mono WAV to the daemon and puts the text at the cursor of the
// message box. Speech plays what the daemon reads aloud for this page: the
// `say` broadcasts that name its listener id. Talk mode streams the
// microphone to the daemon's talk socket as 24 kHz PCM and plays the voice
// that comes back. Each shows only when the daemon says it is ready.
//
// The audio helpers come first and touch no DOM, so they run under node.

import { h, local, setChildren, toast } from "./dom.js";
import { icon, pigLogo } from "./icons.js";
import { api, apiBlob, currentTab, invalidate, params, registerRenderer, request, setSayHandler, state } from "./store.js";

export const DICTATION_RATE = 16_000;
export const TALK_RATE = 24_000;
/** Samples per talk frame: 100 ms at 24 kHz, 4800 bytes. */
export const TALK_FRAME = 2400;

// Audio helpers

function concat(a, b) {
  if (!a.length) return b;
  if (!b.length) return a;
  const out = new Float32Array(a.length + b.length);
  out.set(a);
  out.set(b, a.length);
  return out;
}

/**
 * Converts mono audio between sample rates, chunk by chunk. Going down,
 * each output sample averages the input it covers (a box filter, enough
 * against aliasing for speech); going up, it interpolates.
 */
export class Resampler {
  constructor(from, to) {
    this.ratio = from / to;
    this.buffer = new Float32Array(0);
    this.position = 0;
  }

  push(input) {
    const buffer = concat(this.buffer, input);
    const ratio = this.ratio;
    const out = new Float32Array(Math.max(0, Math.ceil((buffer.length - this.position) / ratio) + 1));
    let t = this.position;
    let n = 0;
    if (ratio >= 1) {
      while (t + ratio <= buffer.length) {
        const start = Math.floor(t);
        const end = Math.max(start + 1, Math.floor(t + ratio));
        let sum = 0;
        for (let i = start; i < end; i += 1) sum += buffer[i];
        out[n++] = sum / (end - start);
        t += ratio;
      }
    } else {
      while (Math.floor(t) + 1 < buffer.length) {
        const i = Math.floor(t);
        const f = t - i;
        out[n++] = buffer[i] * (1 - f) + buffer[i + 1] * f;
        t += ratio;
      }
    }
    const used = Math.min(Math.floor(t), buffer.length);
    this.buffer = buffer.slice(used);
    this.position = t - used;
    return out.slice(0, n);
  }
}

/** A whole recording at another sample rate. */
export function resample(input, from, to) {
  return from === to ? input : new Resampler(from, to).push(input);
}

/** 16-bit little-endian PCM of samples in [-1, 1]. */
export function toPcm16(samples) {
  const view = new DataView(new ArrayBuffer(samples.length * 2));
  for (let i = 0; i < samples.length; i += 1) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(i * 2, Math.round(s < 0 ? s * 0x8000 : s * 0x7fff), true);
  }
  return view.buffer;
}

/** Samples in [-1, 1] of 16-bit little-endian PCM (an odd last byte is ignored). */
export function fromPcm16(buffer) {
  const view = new DataView(buffer);
  const out = new Float32Array(Math.floor(buffer.byteLength / 2));
  for (let i = 0; i < out.length; i += 1) out[i] = view.getInt16(i * 2, true) / 0x8000;
  return out;
}

/** A WAV file of mono 16-bit PCM at `rate`. */
export function wav(pcm, rate) {
  const header = new DataView(new ArrayBuffer(44));
  const ascii = (offset, text) => [...text].forEach((c, i) => header.setUint8(offset + i, c.charCodeAt(0)));
  ascii(0, "RIFF");
  header.setUint32(4, 36 + pcm.byteLength, true);
  ascii(8, "WAVE");
  ascii(12, "fmt ");
  header.setUint32(16, 16, true);
  header.setUint16(20, 1, true);
  header.setUint16(22, 1, true);
  header.setUint32(24, rate, true);
  header.setUint32(28, rate * 2, true);
  header.setUint16(32, 2, true);
  header.setUint16(34, 16, true);
  ascii(36, "data");
  header.setUint32(40, pcm.byteLength, true);
  return new Blob([header.buffer, pcm], { type: "audio/wav" });
}

/** Loudness of a chunk from 0 (-60 dBFS or quieter) to 1 (full scale), for meters. */
export function level(samples) {
  let sum = 0;
  for (let i = 0; i < samples.length; i += 1) sum += samples[i] * samples[i];
  const rms = Math.sqrt(sum / Math.max(1, samples.length));
  return Math.max(0, Math.min(1, (20 * Math.log10(rms || 1e-9) + 60) / 60));
}

/** Cuts a stream of samples into frames of `size`. */
export class Framer {
  constructor(size) {
    this.size = size;
    this.pending = new Float32Array(0);
  }

  push(samples) {
    const buffer = concat(this.pending, samples);
    const frames = [];
    let offset = 0;
    for (; buffer.length - offset >= this.size; offset += this.size) frames.push(buffer.slice(offset, offset + this.size));
    this.pending = buffer.slice(offset);
    return frames;
  }
}

/** `0:07`, `1:30`. */
export function clock(ms) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

// The microphone

const INSECURE = "The microphone works on https or localhost. Open malac through a tunnel (https) or on this machine to use it.";

// Batches the 128-sample render quanta into chunks of 2048 for the page.
const TAP = `
class MalacTap extends AudioWorkletProcessor {
  constructor() {
    super();
    this.chunk = new Float32Array(2048);
    this.filled = 0;
  }
  process(inputs) {
    const input = inputs[0] && inputs[0][0];
    for (let offset = 0; input && offset < input.length; ) {
      const n = Math.min(input.length - offset, this.chunk.length - this.filled);
      this.chunk.set(input.subarray(offset, offset + n), this.filled);
      this.filled += n;
      offset += n;
      if (this.filled === this.chunk.length) {
        this.port.postMessage(this.chunk, [this.chunk.buffer]);
        this.chunk = new Float32Array(2048);
        this.filled = 0;
      }
    }
    return true;
  }
}
registerProcessor("malac-tap", MalacTap);
`;

let tapUrl = null;

/** Why the microphone could not be opened, and what to do about it. */
function micError(error) {
  if (!window.isSecureContext) return INSECURE;
  switch (error?.name) {
    case "NotAllowedError":
    case "SecurityError":
      return params.get("embed") === "1"
        ? "The editor does not let pages use the microphone. Open malac in a browser (malac web --open) to use it."
        : "Microphone access is blocked. Allow it for this site in the browser (the icon in the address bar), then try again.";
    case "NotFoundError":
    case "OverconstrainedError":
      return "No microphone was found. Connect one and try again.";
    case "NotReadableError":
    case "AbortError":
      return "The microphone could not be started. Another app may be using it; close that and try again.";
    default:
      return `The microphone could not be started: ${error?.message ?? error}`;
  }
}

/**
 * Opens the microphone with echo cancellation and noise suppression;
 * `onSamples(chunk, rate)` gets what it hears. Resolves to `{rate, mute, stop}`.
 */
async function openMicrophone(onSamples) {
  if (!window.isSecureContext) throw Object.assign(new Error(INSECURE), { name: "SecurityError" });
  if (!navigator.mediaDevices?.getUserMedia) throw new Error("this browser cannot record audio");
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true },
  });
  const context = new AudioContext();
  try {
    const source = context.createMediaStreamSource(stream);
    // Nodes only run when they lead to the speakers; this one is silent.
    const sink = context.createGain();
    sink.gain.value = 0;
    sink.connect(context.destination);
    let node = null;
    if (context.audioWorklet && typeof AudioWorkletNode === "function") {
      try {
        tapUrl ??= URL.createObjectURL(new Blob([TAP], { type: "text/javascript" }));
        await context.audioWorklet.addModule(tapUrl);
        node = new AudioWorkletNode(context, "malac-tap");
        node.port.onmessage = (event) => onSamples(event.data, context.sampleRate);
      } catch {
        node = null;
      }
    }
    if (!node) {
      // Older browsers: the deprecated ScriptProcessor does the same.
      node = context.createScriptProcessor(4096, 1, 1);
      node.onaudioprocess = (event) => onSamples(new Float32Array(event.inputBuffer.getChannelData(0)), context.sampleRate);
    }
    source.connect(node);
    node.connect(sink);
    await context.resume().catch(() => {});
    return {
      rate: context.sampleRate,
      mute(on) {
        for (const track of stream.getAudioTracks()) track.enabled = !on;
      },
      stop() {
        for (const track of stream.getTracks()) track.stop();
        if (node.port) node.port.onmessage = null;
        else node.onaudioprocess = null;
        source.disconnect();
        node.disconnect();
        context.close().catch(() => {});
      },
    };
  } catch (error) {
    for (const track of stream.getTracks()) track.stop();
    context.close().catch(() => {});
    throw error;
  }
}

/** Whether a feature is on and has a model. */
function voiceReady(feature) {
  const f = state.voice?.[feature];
  return Boolean(f?.enabled && f?.model);
}

const isMac = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform ?? "");
const DICTATE_KEYS = isMac ? "⌘⇧Space" : "Ctrl+Shift+Space";

/** A five-bar level meter; `set(level)` lights the bars. */
function meter(className = "") {
  const bars = Array.from({ length: 5 }, () => h("span", { class: "level-bar" }));
  const el = h("span", { class: `level-meter ${className}`.trim(), "aria-hidden": "true" }, bars);
  return {
    el,
    set(value) {
      bars.forEach((bar, i) => bar.classList.toggle("on", value >= (i + 0.5) / bars.length));
    },
  };
}

// Page elements, from initVoice.
const els = {};

// Dictation

const MAX_DICTATION_MS = 10 * 60 * 1000;
const MIN_DICTATION_SAMPLES = DICTATION_RATE * 0.3;

const dictation = { state: "idle", mic: null, chunks: [], rate: 0, length: 0, started: 0, level: 0, frame: 0, run: 0, meter: null, shown: null };

async function startDictation() {
  if (dictation.state !== "idle" || !voiceReady("dictation")) return;
  if (!window.isSecureContext) return toast(INSECURE, "error");
  if (talk.socket) return toast("End talk mode to dictate.", "warning");
  const run = ++dictation.run;
  Object.assign(dictation, { state: "starting", chunks: [], length: 0, level: 0 });
  document.addEventListener("keydown", onDictationKey, true);
  renderDictation();
  let mic;
  try {
    mic = await openMicrophone((samples, rate) => {
      if (dictation.run !== run || dictation.state !== "recording") return;
      dictation.chunks.push(samples);
      dictation.rate = rate;
      dictation.length += samples.length;
      dictation.level = level(samples);
    });
  } catch (error) {
    if (dictation.run !== run) return;
    dictation.state = "idle";
    document.removeEventListener("keydown", onDictationKey, true);
    renderDictation();
    toast(micError(error), "error");
    return;
  }
  if (dictation.run !== run) return mic.stop();
  Object.assign(dictation, { state: "recording", mic, rate: mic.rate, started: performance.now() });
  renderDictation();
  tickDictation();
}

function tickDictation() {
  if (dictation.state !== "recording") return;
  const elapsed = performance.now() - dictation.started;
  if (elapsed >= MAX_DICTATION_MS) {
    toast("Dictation stops after ten minutes; the recording so far is being transcribed.", "info");
    stopDictation();
    return;
  }
  dictation.meter?.set(dictation.level);
  const time = els.status?.querySelector(".dictation-time");
  if (time) time.textContent = clock(elapsed);
  dictation.frame = requestAnimationFrame(tickDictation);
}

function releaseMic() {
  cancelAnimationFrame(dictation.frame);
  dictation.mic?.stop();
  dictation.mic = null;
}

/** Stops recording and puts what was said at the cursor. */
async function stopDictation() {
  if (dictation.state !== "recording") return;
  const run = dictation.run;
  const { chunks, rate, length } = dictation;
  releaseMic();
  dictation.state = "transcribing";
  renderDictation();
  try {
    const recording = new Float32Array(length);
    let offset = 0;
    for (const chunk of chunks) {
      recording.set(chunk, offset);
      offset += chunk.length;
    }
    const samples = resample(recording, rate, DICTATION_RATE);
    if (samples.length < MIN_DICTATION_SAMPLES) {
      toast("That was too short to hear anything. Speak, then stop the recording.", "warning");
      return;
    }
    const reply = await api("/api/voice/transcribe", { method: "POST", body: wav(toPcm16(samples), DICTATION_RATE) });
    if (dictation.run !== run) return;
    const text = String(reply?.text ?? "").trim();
    if (text) insertText(text);
    else toast("Nothing was heard. Check that the right microphone is selected and try again.", "warning");
  } catch (error) {
    if (dictation.run === run) toast(`Dictation failed: ${error.message}`, "error");
  } finally {
    if (dictation.run === run) {
      dictation.state = "idle";
      document.removeEventListener("keydown", onDictationKey, true);
      renderDictation();
    }
  }
}

function cancelDictation() {
  if (dictation.state === "idle") return;
  dictation.run += 1;
  releaseMic();
  dictation.state = "idle";
  document.removeEventListener("keydown", onDictationKey, true);
  renderDictation();
}

function toggleDictation() {
  if (dictation.state === "recording") stopDictation();
  else if (dictation.state === "idle") startDictation();
}

// While recording, Enter inserts and Escape throws the recording away;
// until the text is in, Enter does not send the message.
function onDictationKey(event) {
  if (event.isComposing) return;
  if (event.key === "Escape") {
    cancelDictation();
  } else if (event.key === "Enter" && !event.shiftKey) {
    const target = event.target;
    // Enter on another button still presses that button.
    if (target !== els.input && target !== els.dictate && target !== document.body) return;
    if (dictation.state === "recording") stopDictation();
  } else {
    return;
  }
  event.preventDefault();
  event.stopPropagation();
}

/** Puts `text` at the cursor of the message box, spaced from what is around it. */
function insertText(text) {
  const input = els.input;
  const start = input.selectionStart ?? input.value.length;
  const end = input.selectionEnd ?? start;
  const before = input.value.slice(0, start);
  const after = input.value.slice(end);
  const lead = before && !/\s$/.test(before) ? " " : "";
  const trail = after && !/^\s/.test(after) ? " " : "";
  input.setRangeText(`${lead}${text}${trail}`, start, end, "end");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.focus();
}

function renderDictation() {
  const button = els.dictate;
  if (!button) return;
  const { state: phase } = dictation;
  const key = `${phase}:${window.isSecureContext}`;
  if (dictation.shown === key) return;
  dictation.shown = key;
  const active = phase !== "idle";
  els.composer.classList.toggle("dictating", active);
  button.classList.toggle("recording", phase === "recording");
  button.disabled = phase === "transcribing";
  button.setAttribute("aria-pressed", String(phase === "recording"));
  const label = phase === "recording" ? "Stop and insert the text (Enter)" : `Dictate (${DICTATE_KEYS})`;
  button.setAttribute("aria-label", window.isSecureContext ? label : "Dictate (needs https or localhost)");
  button.title = button.getAttribute("aria-label");
  setChildren(button, phase === "recording" ? icon("stop", 14) : phase === "idle" ? icon("mic", 16) : h("span", { class: "spinner small" }));
  els.status.hidden = !active;
  dictation.meter = null;
  if (phase === "starting") {
    setChildren(els.status, h("span", { class: "dictation-text" }, "Starting the microphone…"));
  } else if (phase === "recording") {
    dictation.meter = meter();
    setChildren(
      els.status,
      h("span", { class: "dictation-dot", "aria-hidden": "true" }),
      dictation.meter.el,
      // The time changes every frame; screen readers hear the words only.
      h("span", { class: "dictation-time tabular", "aria-hidden": "true" }, clock(performance.now() - dictation.started)),
      h("span", { class: "dictation-text" }, "Listening. ", h("kbd", {}, "Enter"), " inserts, ", h("kbd", {}, "Esc"), " cancels"),
    );
  } else if (phase === "transcribing") {
    setChildren(els.status, h("span", { class: "dictation-text" }, "Transcribing…"));
  } else {
    setChildren(els.status);
  }
}

// Speech: sentences the daemon reads aloud for this page.

const SPEAK_KEY = "malac.speak";
const QUEUE_MAX = 5;

const speech = { on: false, socket: null, listener: null, busy: false, queue: [], playing: false, context: null, source: null, current: null, prompted: 0 };

/** "Speak on this device", remembered in this browser; on by default. */
export function speakHere() {
  return local.get(SPEAK_KEY) !== "off";
}

export function setSpeakHere(on) {
  local.set(SPEAK_KEY, on ? null : "off");
  if (!on) stopSpeech();
  syncListening();
  invalidate("voice", "settings");
}

// The daemon plays speech on connections that asked for it; ask again on
// every new connection, and stop asking when switched off.
function syncListening() {
  const socket = state.socket?.readyState === WebSocket.OPEN ? state.socket : null;
  const want = Boolean(socket) && speakHere() && voiceReady("speech");
  if (speech.busy || (want === speech.on && socket === speech.socket)) return;
  if (!socket) {
    Object.assign(speech, { on: false, socket: null, listener: null });
    return;
  }
  speech.busy = true;
  request({ type: "voice_listen", on: want })
    .then((reply) => (speech.listener = reply.listener ?? null))
    .catch(() => (speech.listener = null))
    .finally(() => {
      Object.assign(speech, { on: want, socket, busy: false });
      syncListening();
    });
}

function speechContext() {
  speech.context ??= new AudioContext();
  return speech.context;
}

function onSay(message) {
  if (message.type === "say_cancelled") return cancelSpeech(message.ids ?? []);
  if (speech.listener == null || message.listener !== speech.listener || !speakHere() || talk.socket) return;
  // The daemon makes the audio while the page fetches it.
  const audio = apiBlob(`/api/voice/say/${encodeURIComponent(message.id)}`);
  audio.catch(() => {});
  speech.queue.push({ id: message.id, text: message.text, audio });
  if (speech.queue.length > QUEUE_MAX) speech.queue.splice(0, speech.queue.length - QUEUE_MAX);
  playQueue();
}

// Browsers start audio only after the user has interacted with the page.
async function soundAllowed(context) {
  if (context.state === "running") return true;
  await Promise.race([context.resume().catch(() => {}), new Promise((resolve) => setTimeout(resolve, 300))]);
  return context.state === "running";
}

async function playQueue() {
  if (speech.playing) return;
  speech.playing = true;
  try {
    while (speech.queue.length) {
      const context = speechContext();
      if (!(await soundAllowed(context))) {
        askForSound();
        return;
      }
      const item = speech.queue.shift();
      speech.current = item.id;
      toast(item.text, "info", { duration: Math.min(12_000, 3000 + item.text.length * 60) });
      try {
        const buffer = await context.decodeAudioData(await (await item.audio).arrayBuffer());
        // Cancelled while its audio was still on the way.
        if (speech.current !== item.id) continue;
        await new Promise((resolve) => {
          const source = context.createBufferSource();
          source.buffer = buffer;
          source.connect(context.destination);
          source.onended = resolve;
          speech.source = source;
          source.start();
        });
      } catch (error) {
        console.warn("speech", error);
      } finally {
        speech.source = null;
        speech.current = null;
      }
    }
  } finally {
    speech.playing = false;
  }
}

function askForSound() {
  if (Date.now() - speech.prompted < 30_000) return;
  speech.prompted = Date.now();
  toast("malac wants to speak, and this browser needs a click before it plays sound.", "info", {
    duration: 30_000,
    action: {
      label: "Enable sound",
      run: () => {
        speech.prompted = 0;
        speechContext().resume().then(playQueue, playQueue);
      },
    },
  });
}

function stopSpeech() {
  speech.queue = [];
  speech.current = null;
  stopSource();
}

// The user answered an approval or sent a prompt: those sentences stop now.
function cancelSpeech(ids) {
  speech.queue = speech.queue.filter((item) => !ids.includes(item.id));
  if (speech.current && ids.includes(speech.current)) {
    speech.current = null;
    stopSource();
  }
}

function stopSource() {
  try {
    speech.source?.stop();
  } catch {
    // It had ended.
  }
}

// Talk mode

const TALK_STATES = {
  connecting: "Connecting",
  listening: "Listening",
  thinking: "Thinking",
  speaking: "Speaking",
  ended: "Ended",
};

const talk = { socket: null, panel: null, mic: null, phase: "idle", ready: false, muted: false, done: true, context: null, output: null, analyser: null, sources: new Set(), next: 0, carry: null, resampler: null, framer: null, level: 0, frame: 0, line: null, error: null, previous: null, meter: null, refs: null };

/** Whether talk mode is open. */
function talking() {
  return talk.panel != null;
}

function talkUrl() {
  const query = new URLSearchParams();
  const tab = currentTab();
  if (tab) query.set("tab", tab.id);
  if (state.selected && tab?.agents.includes(state.selected)) query.set("agent", state.selected);
  query.set("token", state.token);
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${location.host}/api/voice/talk?${query}`;
}

function startTalk() {
  if (talk.socket || !voiceReady("talk")) return;
  if (!window.isSecureContext) return toast(INSECURE, "error");
  cancelDictation();
  stopSpeech();
  // "Talk again" keeps the button that first opened the panel.
  const previous = talk.panel ? talk.previous : document.activeElement;
  openTalkPanel();
  talk.previous = previous;
  // Created in the click, so the browser lets it play.
  talk.context = new AudioContext();
  talk.context.resume().catch(() => {});
  talk.output = talk.context.createGain();
  talk.analyser = talk.context.createAnalyser();
  talk.analyser.fftSize = 512;
  talk.output.connect(talk.analyser);
  talk.output.connect(talk.context.destination);
  Object.assign(talk, { ready: false, muted: false, done: true, next: 0, carry: null, error: null, line: null, level: 0 });
  setTalkPhase("connecting");
  const socket = new WebSocket(talkUrl());
  socket.binaryType = "arraybuffer";
  talk.socket = socket;
  let opened = false;
  socket.addEventListener("open", () => (opened = true));
  socket.addEventListener("message", (event) => {
    if (talk.socket !== socket) return;
    if (typeof event.data === "string") talkMessage(JSON.parse(event.data));
    else playTalkAudio(event.data);
  });
  socket.addEventListener("close", () => {
    if (talk.socket !== socket) return;
    endTalk(talk.error ?? (opened ? "The connection to malac closed." : "Talk mode could not connect to malac. Check that the daemon runs, then try again."));
  });
  // Opening the microphone now puts the permission prompt up while the
  // voice model connects; frames go out once it is ready.
  openMicrophone((samples, rate) => talkSamples(socket, samples, rate))
    .then((mic) => {
      if (talk.socket !== socket) return mic.stop();
      talk.mic = mic;
      talk.resampler = new Resampler(mic.rate, TALK_RATE);
      talk.framer = new Framer(TALK_FRAME);
      mic.mute(talk.muted);
    })
    .catch((error) => {
      if (talk.socket === socket) endTalk(micError(error), "error");
    });
  tickTalk();
}

function talkSamples(socket, samples, rate) {
  if (talk.socket !== socket || !talk.resampler) return;
  talk.level = talk.muted ? 0 : level(samples);
  if (!talk.ready || talk.muted || socket.readyState !== WebSocket.OPEN) return;
  for (const frame of talk.framer.push(talk.resampler.push(samples))) socket.send(toPcm16(frame));
}

function talkMessage(message) {
  switch (message.type) {
    case "ready":
      talk.ready = true;
      talk.refs.meta.textContent = [message.model, message.voice].filter(Boolean).join(" · ");
      setTalkPhase("listening");
      break;
    case "listening":
      // The user talks over the voice: it stops at once.
      stopTalkAudio();
      talk.line = null;
      setTalkPhase("listening");
      break;
    case "thinking":
      setTalkPhase("thinking");
      break;
    case "transcript":
      talkTranscript(message.role, String(message.text ?? ""), message.final);
      break;
    case "tool":
      talk.line = null;
      addTalkLine("tool", message.summary || message.name);
      break;
    case "done":
      talk.done = true;
      talk.line = null;
      if (!talk.sources.size && talk.phase !== "connecting") setTalkPhase("listening");
      break;
    case "error":
      talk.error = sentence(message.message || "something went wrong");
      addTalkLine("error", talk.error);
      break;
    case "closed":
      endTalk(talk.error ?? sentence(message.reason || "talk mode ended"));
      break;
  }
}

/** `talk mode ended` as `Talk mode ended.` */
function sentence(text) {
  const value = String(text).trim();
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}${/[.!?]$/.test(value) ? "" : "."}`;
}

function talkTranscript(role, text, final) {
  if (role === "user") {
    talk.line = null;
    if (text.trim()) addTalkLine("user", text.trim());
    return;
  }
  if (!talk.line) {
    if (!text.trim()) return;
    talk.line = addTalkLine("assistant", "");
  }
  const body = talk.line.querySelector(".talk-text");
  body.textContent = final && text.trim() ? text.trim() : body.textContent + text;
  if (final) talk.line = null;
  scrollTalkLog();
}

function playTalkAudio(data) {
  const context = talk.context;
  if (!context) return;
  let bytes = new Uint8Array(data);
  if (talk.carry != null) {
    const joined = new Uint8Array(bytes.length + 1);
    joined[0] = talk.carry;
    joined.set(bytes, 1);
    bytes = joined;
    talk.carry = null;
  }
  if (bytes.length % 2) {
    talk.carry = bytes[bytes.length - 1];
    bytes = bytes.subarray(0, bytes.length - 1);
  }
  const samples = fromPcm16(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length));
  if (!samples.length) return;
  const buffer = context.createBuffer(1, samples.length, TALK_RATE);
  buffer.copyToChannel(samples, 0);
  const source = context.createBufferSource();
  source.buffer = buffer;
  source.connect(talk.output);
  // Chunks play back to back; a late one starts a moment from now.
  const at = Math.max(talk.next, context.currentTime + 0.05);
  source.start(at);
  talk.next = at + buffer.duration;
  talk.sources.add(source);
  talk.done = false;
  source.onended = () => {
    talk.sources.delete(source);
    if (!talk.sources.size && talk.done && talk.phase === "speaking") setTalkPhase("listening");
  };
  if (talk.phase !== "speaking") setTalkPhase("speaking");
}

function stopTalkAudio() {
  for (const source of talk.sources) {
    source.onended = null;
    try {
      source.stop();
    } catch {
      // Not started yet or already done.
    }
  }
  talk.sources.clear();
  talk.next = 0;
}

function tickTalk() {
  if (!talk.panel) return;
  let value = talk.level;
  if (talk.phase === "speaking" && talk.analyser) {
    const data = new Float32Array(talk.analyser.fftSize);
    talk.analyser.getFloatTimeDomainData(data);
    value = level(data);
  } else if (talk.phase === "connecting" || talk.phase === "ended") {
    value = 0;
  }
  talk.meter?.set(value);
  talk.frame = requestAnimationFrame(tickTalk);
}

function setTalkPhase(phase) {
  talk.phase = phase;
  if (!talk.refs) return;
  const { status, panel } = talk.refs;
  panel.dataset.phase = phase;
  setChildren(
    status,
    phase === "connecting" ? h("span", { class: "spinner small" }) : h("span", { class: `dot ${phase === "ended" ? "idle" : phase === "listening" ? "ok" : "running"}` }),
    h("span", {}, talk.muted && phase === "listening" ? "Muted" : TALK_STATES[phase]),
  );
  renderTalkActions();
}

function toggleMute() {
  talk.muted = !talk.muted;
  talk.mic?.mute(talk.muted);
  setTalkPhase(talk.phase);
}

/** Ends talk mode from this side. */
function stopTalk() {
  const socket = talk.socket;
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "stop" }));
  talk.socket = null;
  socket?.close();
  releaseTalk();
  closeTalkPanel();
}

function releaseTalk() {
  talk.mic?.stop();
  talk.mic = null;
  talk.resampler = null;
  stopTalkAudio();
  talk.context?.close().catch(() => {});
  Object.assign(talk, { context: null, output: null, analyser: null, ready: false, level: 0 });
}

/** The conversation ended on the daemon's side or failed: say why and keep the panel. */
function endTalk(reason, kind = "note") {
  if (!talk.panel || talk.phase === "ended") return;
  const socket = talk.socket;
  talk.socket = null;
  socket?.close();
  releaseTalk();
  // An error from the daemon is on screen already.
  if (reason !== talk.error) addTalkLine(kind, reason || "Talk mode ended.");
  setTalkPhase("ended");
  talk.refs.panel.querySelector(".talk-actions .button.primary")?.focus();
}

function addTalkLine(kind, text) {
  const { log, empty } = talk.refs;
  empty.hidden = true;
  const who = { user: "You", assistant: "Voice" }[kind];
  const line = h(
    "div",
    { class: "talk-line", "data-kind": kind },
    kind === "tool" ? pigLogo(14) : kind === "error" ? icon("alert", 14) : null,
    who ? h("span", { class: "talk-who" }, who) : null,
    h("span", { class: "talk-text" }, text),
  );
  log.append(line);
  scrollTalkLog();
  return line;
}

function scrollTalkLog() {
  const log = talk.refs?.log;
  if (log) log.scrollTop = log.scrollHeight;
}

function openTalkPanel() {
  if (talk.panel) {
    cancelAnimationFrame(talk.frame);
    talk.panel.remove();
  }
  const status = h("span", { class: "talk-status", role: "status" });
  const meta = h("span", { class: "talk-meta" });
  const log = h("div", { class: "talk-log", role: "log", "aria-label": "Conversation", tabindex: "0" });
  const empty = h("p", { class: "talk-empty" }, "Say what you want malac to do. The voice passes it on and tells you what happened.");
  log.append(empty);
  const actions = h("div", { class: "talk-actions" });
  const mute = h("button", { class: "button talk-mute", type: "button", onclick: toggleMute });
  talk.meter = meter("large");
  const panel = h(
    "section",
    { class: "talk-panel", role: "dialog", "aria-label": "Talk mode", tabindex: "-1" },
    h("header", { class: "talk-head" }, icon("headset", 16), h("h2", { class: "talk-title" }, "Talk mode"), status, h("span", { class: "talk-spacer" }), talk.meter.el),
    log,
    h("footer", { class: "talk-foot" }, meta, actions),
  );
  panel.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    if (talk.phase === "ended") closeTalkPanel();
    else stopTalk();
  });
  talk.panel = panel;
  talk.refs = { panel, status, meta, log, empty, actions, mute };
  document.body.append(panel);
  document.body.classList.add("talking");
  panel.focus();
  renderVoiceControls();
}

// Mute and End while talking; Talk again and Close once it ended. The
// buttons are kept, not redrawn, so keyboard focus stays where it is.
function renderTalkActions() {
  const { actions, mute } = talk.refs;
  const ended = talk.phase === "ended";
  if (actions.dataset.ended !== String(ended)) {
    actions.dataset.ended = String(ended);
    setChildren(
      actions,
      ended
        ? [
            h("button", { class: "button", type: "button", onclick: () => startTalk() }, icon("headset", 14), "Talk again"),
            h("button", { class: "button primary", type: "button", onclick: closeTalkPanel }, "Close"),
          ]
        : [mute, h("button", { class: "button primary", type: "button", title: "End talk mode (Esc)", onclick: stopTalk }, "End")],
    );
  }
  mute.setAttribute("aria-pressed", String(talk.muted));
  mute.title = talk.muted ? "Turn the microphone back on" : "Turn the microphone off";
  setChildren(mute, icon(talk.muted ? "micOff" : "mic", 14), talk.muted ? "Unmute" : "Mute");
}

function closeTalkPanel() {
  if (!talk.panel) return;
  if (talk.socket) return stopTalk();
  cancelAnimationFrame(talk.frame);
  talk.panel.remove();
  talk.panel = null;
  talk.refs = null;
  talk.meter = null;
  talk.phase = "idle";
  document.body.classList.remove("talking");
  renderVoiceControls();
  const previous = talk.previous;
  talk.previous = null;
  if (previous?.isConnected && !previous.hidden) previous.focus({ preventScroll: true });
  else els.input?.focus({ preventScroll: true });
}

// Page controls

let controlsKey = null;

/** Shows the microphone, talk and speech buttons that apply now. */
export function renderVoiceControls() {
  if (!els.dictate) return;
  renderDictation();
  const dictate = voiceReady("dictation");
  const on = speakHere();
  const key = JSON.stringify([dictate, voiceReady("talk"), Boolean(currentTab()), talking(), voiceReady("speech"), on]);
  if (controlsKey === key) return;
  controlsKey = key;
  if (!dictate && dictation.state !== "idle") cancelDictation();
  els.dictate.hidden = !dictate;
  els.talk.hidden = !voiceReady("talk") || !currentTab() || talking();
  els.talk.setAttribute("aria-label", window.isSecureContext ? "Talk mode" : "Talk mode (needs https or localhost)");
  els.talk.title = els.talk.getAttribute("aria-label");
  els.speak.hidden = !voiceReady("speech");
  els.speak.setAttribute("aria-pressed", String(on));
  els.speak.setAttribute("aria-label", "Speak on this device");
  els.speak.title = on ? "Speech plays on this device. Click to stop it here." : "Speech is off on this device. Click to play it here.";
  setChildren(els.speak, icon(on ? "volume" : "volumeOff", 16));
}

function renderVoice() {
  renderVoiceControls();
  syncListening();
}

/** Wires the composer's microphone and talk buttons and the header's speech switch. */
export function initVoice({ composer, input, dictate, talk: talkButton, status, speak }) {
  Object.assign(els, { composer, input, dictate, talk: talkButton, status, speak });
  setChildren(talkButton, icon("headset", 16));
  dictate.addEventListener("click", toggleDictation);
  talkButton.addEventListener("click", startTalk);
  speak.addEventListener("click", () => {
    const on = !speakHere();
    setSpeakHere(on);
    toast(on ? "Speech plays on this device." : "Speech is off on this device.");
  });
  registerRenderer("voice", renderVoice);
  setSayHandler(onSay);
  document.addEventListener("keydown", (event) => {
    if (event.code !== "Space" || !event.shiftKey || !(event.ctrlKey || event.metaKey) || event.altKey) return;
    if (document.querySelector(".modal-backdrop, .viewer") || !voiceReady("dictation") || !input.isConnected) return;
    event.preventDefault();
    if (dictation.state === "idle") input.focus();
    toggleDictation();
  });
  // A click or key anywhere lets speech play later without asking.
  const unlock = () => {
    if (speech.on && speakHere() && speechContext().state === "suspended") speechContext().resume().then(playQueue, () => {});
  };
  document.addEventListener("pointerdown", unlock, true);
  document.addEventListener("keydown", unlock, true);
  renderVoiceControls();
}
