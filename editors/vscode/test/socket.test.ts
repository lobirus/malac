import assert from "node:assert/strict";
import * as http from "node:http";
import { createHash } from "node:crypto";
import type { Socket } from "node:net";
import { test } from "node:test";
import { encodeFrame, FrameReader, OPCODE, openSocket, redact } from "../src/connection/socket";

test("client frames are masked and read back", () => {
  const reader = new FrameReader();
  const frame = encodeFrame(OPCODE.text, Buffer.from("héllo"), Buffer.from([1, 2, 3, 4]));
  assert.equal(frame[1] & 0x80, 0x80, "masked");
  const [decoded] = reader.push(frame);
  assert.equal(decoded.payload.toString(), "héllo");
});

test("medium and large lengths, split across chunks", () => {
  for (const size of [125, 126, 70_000]) {
    const reader = new FrameReader();
    const frame = encodeFrame(OPCODE.text, Buffer.alloc(size, 0x61));
    const frames = [...reader.push(frame.subarray(0, 3)), ...reader.push(frame.subarray(3, 50)), ...reader.push(frame.subarray(50))];
    assert.equal(frames.length, 1);
    assert.equal(frames[0].payload.length, size);
  }
});

test("fragmented messages are joined; control frames pass between them", () => {
  const unmasked = (fin: boolean, opcode: number, text: string) => Buffer.concat([Buffer.from([(fin ? 0x80 : 0) | opcode, text.length]), Buffer.from(text)]);
  const reader = new FrameReader();
  const frames = reader.push(Buffer.concat([unmasked(false, OPCODE.text, "he"), unmasked(true, OPCODE.ping, "p"), unmasked(true, OPCODE.continuation, "llo")]));
  assert.deepEqual(frames.map((f) => [f.opcode, f.payload.toString()]), [
    [OPCODE.ping, "p"],
    [OPCODE.text, "hello"],
  ]);
});

test("tokens are kept out of messages", () => {
  assert.equal(redact("ws://127.0.0.1:7700/api/ws?token=secret&version=1"), "ws://127.0.0.1:7700/api/ws?token=…&version=1");
});

/** A one-shot WebSocket echo server, enough to exercise the built-in client. */
function echoServer(): Promise<{ url: string; close: () => void }> {
  const server = http.createServer();
  server.on("upgrade", (request, socket: Socket) => {
    const accept = createHash("sha1").update(`${request.headers["sec-websocket-key"]}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    // The first message arrives with the handshake, as a daemon's snapshot can.
    socket.write(Buffer.concat([Buffer.from([0x81, 5]), Buffer.from("hello")]));
    const reader = new FrameReader();
    socket.on("data", (chunk: Buffer) => {
      for (const frame of reader.push(chunk)) {
        if (frame.opcode === OPCODE.text) {
          const text = `echo:${frame.payload.toString()}`;
          socket.write(Buffer.concat([Buffer.from([0x81, text.length]), Buffer.from(text)]));
        } else if (frame.opcode === OPCODE.close) {
          socket.end(Buffer.from([0x88, 0]));
        }
      }
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => {
    const { port } = server.address() as { port: number };
    resolve({ url: `ws://127.0.0.1:${port}/api/ws?token=t`, close: () => server.close() });
  }));
}

for (const forceFallback of [true, false]) {
  test(`the ${forceFallback ? "built-in" : "runtime"} client talks to a server`, async () => {
    const server = await echoServer();
    const received: string[] = [];
    let closed = "";
    const socket = await openSocket(server.url, { onMessage: (text) => received.push(text), onClose: (reason) => (closed = reason) }, { forceFallback });
    socket.send("ping");
    await new Promise((r) => setTimeout(r, 100));
    assert.deepEqual(received, ["hello", "echo:ping"]);
    socket.close();
    await new Promise((r) => setTimeout(r, 50));
    assert.ok(closed !== "" || !forceFallback);
    server.close();
  });
}
