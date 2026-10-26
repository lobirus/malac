// A text-only WebSocket client. Uses the runtime's WebSocket when it has one
// (Node 22+, recent VS Code) and otherwise a small RFC 6455 client over
// node:http, so older extension hosts work without a dependency.

import { createHash, randomBytes } from "node:crypto";
import * as http from "node:http";
import * as https from "node:https";
import type { Socket } from "node:net";
import type { Duplex } from "node:stream";

export interface TextSocket {
  send(text: string): void;
  close(): void;
}

export interface SocketHandlers {
  onMessage(text: string): void;
  onClose(reason: string): void;
}

const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const CONNECT_TIMEOUT_MS = 10_000;

/** Opens a socket; resolves once the handshake is done. */
export function openSocket(url: string, handlers: SocketHandlers, { forceFallback = false } = {}): Promise<TextSocket> {
  const Native = (globalThis as { WebSocket?: typeof WebSocket }).WebSocket;
  if (Native && !forceFallback) return openNative(Native, url, handlers);
  return openFallback(url, handlers);
}

function openNative(Native: typeof WebSocket, url: string, handlers: SocketHandlers): Promise<TextSocket> {
  return new Promise((resolve, reject) => {
    const socket = new Native(url);
    let open = false;
    const timer = setTimeout(() => {
      if (!open) socket.close();
      reject(new Error("timed out connecting to the daemon"));
    }, CONNECT_TIMEOUT_MS);
    socket.addEventListener("open", () => {
      open = true;
      clearTimeout(timer);
      resolve({ send: (text) => socket.send(text), close: () => socket.close() });
    });
    socket.addEventListener("message", (event) => {
      if (typeof event.data === "string") handlers.onMessage(event.data);
    });
    socket.addEventListener("error", () => {
      if (!open) {
        clearTimeout(timer);
        reject(new Error(`cannot connect to ${redact(url)}`));
      }
    });
    socket.addEventListener("close", (event) => {
      if (open) handlers.onClose(event.reason || `closed (${event.code})`);
    });
  });
}

function openFallback(url: string, handlers: SocketHandlers): Promise<TextSocket> {
  const target = new URL(url);
  const secure = target.protocol === "wss:";
  const key = randomBytes(16).toString("base64");
  const request = (secure ? https : http).request({
    host: target.hostname,
    port: target.port || (secure ? 443 : 80),
    path: `${target.pathname}${target.search}`,
    headers: {
      Connection: "Upgrade",
      Upgrade: "websocket",
      "Sec-WebSocket-Key": key,
      "Sec-WebSocket-Version": "13",
    },
    timeout: CONNECT_TIMEOUT_MS,
  });
  return new Promise((resolve, reject) => {
    request.on("timeout", () => request.destroy(new Error("timed out connecting to the daemon")));
    request.on("error", (error) => reject(error));
    request.on("response", (response) => {
      response.resume();
      reject(new Error(`the daemon refused the connection (HTTP ${response.statusCode})`));
    });
    request.on("upgrade", (response, stream: Duplex, head: Buffer) => {
      const expected = createHash("sha1").update(key + GUID).digest("base64");
      if (response.headers["sec-websocket-accept"] !== expected) {
        stream.destroy();
        reject(new Error("bad WebSocket handshake"));
        return;
      }
      resolve(attach(stream, head, handlers));
    });
    request.end();
  });
}

function attach(stream: Duplex, head: Buffer, handlers: SocketHandlers): TextSocket {
  const reader = new FrameReader();
  let closed = false;
  const finish = (reason: string) => {
    if (closed) return;
    closed = true;
    stream.destroy();
    handlers.onClose(reason);
  };
  const consume = (chunk: Buffer) => {
    let frames: Frame[];
    try {
      frames = reader.push(chunk);
    } catch (error) {
      finish(error instanceof Error ? error.message : String(error));
      return;
    }
    for (const frame of frames) {
      if (frame.opcode === OPCODE.text) handlers.onMessage(frame.payload.toString("utf8"));
      else if (frame.opcode === OPCODE.ping) stream.write(encodeFrame(OPCODE.pong, frame.payload));
      else if (frame.opcode === OPCODE.close) {
        if (!closed) stream.write(encodeFrame(OPCODE.close, frame.payload.subarray(0, 2)));
        finish(frame.payload.length > 2 ? frame.payload.subarray(2).toString("utf8") : "closed by the daemon");
      }
    }
  };
  (stream as Partial<Socket>).setNoDelay?.(true);
  stream.on("data", consume);
  stream.on("close", () => finish("connection lost"));
  stream.on("error", (error) => finish(error.message));
  if (head.length) consume(head);
  return {
    send(text) {
      if (!closed) stream.write(encodeFrame(OPCODE.text, Buffer.from(text, "utf8")));
    },
    close() {
      if (closed) return;
      stream.write(encodeFrame(OPCODE.close, Buffer.from([0x03, 0xe8])));
      stream.end();
      finish("closed");
    },
  };
}

export const OPCODE = { continuation: 0x0, text: 0x1, binary: 0x2, close: 0x8, ping: 0x9, pong: 0xa } as const;

export interface Frame {
  opcode: number;
  payload: Buffer;
}

/** A client frame: final, masked (`mask` is for tests). */
export function encodeFrame(opcode: number, payload: Buffer, mask: Buffer = randomBytes(4)): Buffer {
  const length = payload.length;
  const header = length < 126 ? Buffer.alloc(2) : length < 65536 ? Buffer.alloc(4) : Buffer.alloc(10);
  header[0] = 0x80 | opcode;
  if (length < 126) {
    header[1] = 0x80 | length;
  } else if (length < 65536) {
    header[1] = 0x80 | 126;
    header.writeUInt16BE(length, 2);
  } else {
    header[1] = 0x80 | 127;
    header.writeBigUInt64BE(BigInt(length), 2);
  }
  const body = Buffer.alloc(length);
  for (let i = 0; i < length; i += 1) body[i] = payload[i] ^ mask[i % 4];
  return Buffer.concat([header, mask, body]);
}

/** Reassembles frames (and fragmented messages) from a byte stream. */
export class FrameReader {
  private buffer: Buffer = Buffer.alloc(0);
  private fragments: Buffer[] = [];
  private fragmentOpcode = 0;

  push(chunk: Buffer): Frame[] {
    this.buffer = this.buffer.length ? Buffer.concat([this.buffer, chunk]) : chunk;
    const frames: Frame[] = [];
    for (;;) {
      const frame = this.next();
      if (!frame) break;
      const { fin, opcode, payload } = frame;
      if (opcode >= 0x8) {
        frames.push({ opcode, payload });
      } else if (opcode === OPCODE.continuation) {
        this.fragments.push(payload);
        if (fin) {
          frames.push({ opcode: this.fragmentOpcode, payload: Buffer.concat(this.fragments) });
          this.fragments = [];
        }
      } else if (fin) {
        frames.push({ opcode, payload });
      } else {
        this.fragmentOpcode = opcode;
        this.fragments = [payload];
      }
    }
    return frames;
  }

  private next(): { fin: boolean; opcode: number; payload: Buffer } | null {
    const data = this.buffer;
    if (data.length < 2) return null;
    const fin = (data[0] & 0x80) !== 0;
    const opcode = data[0] & 0x0f;
    const masked = (data[1] & 0x80) !== 0;
    let length = data[1] & 0x7f;
    let offset = 2;
    if (length === 126) {
      if (data.length < 4) return null;
      length = data.readUInt16BE(2);
      offset = 4;
    } else if (length === 127) {
      if (data.length < 10) return null;
      const big = data.readBigUInt64BE(2);
      if (big > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error("frame too large");
      length = Number(big);
      offset = 10;
    }
    const maskOffset = offset;
    if (masked) offset += 4;
    if (data.length < offset + length) return null;
    let payload = data.subarray(offset, offset + length);
    if (masked) {
      const mask = data.subarray(maskOffset, maskOffset + 4);
      payload = Buffer.from(payload.map((byte, i) => byte ^ mask[i % 4]));
    }
    this.buffer = data.subarray(offset + length);
    return { fin, opcode, payload };
  }
}

/** The URL without its token, for messages. */
export function redact(url: string): string {
  return url.replace(/([?&]token=)[^&]+/, "$1…");
}
