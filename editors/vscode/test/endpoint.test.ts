import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { after, test } from "node:test";
import { LocalEndpoints, parseConnectionSetting, readConnections, SavedEndpoints } from "../src/connection/endpoint";

const made: string[] = [];
after(() => {
  for (const dir of made) fs.rmSync(dir, { recursive: true, force: true });
});

function home(connections?: unknown): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "malac-endpoint-"));
  made.push(dir);
  if (connections !== undefined) fs.writeFileSync(path.join(dir, "connections.json"), JSON.stringify(connections));
  return dir;
}

test("malac.connection is local or the name of a saved connection", () => {
  assert.deepEqual(parseConnectionSetting(undefined), { kind: "local" });
  assert.deepEqual(parseConnectionSetting(" local "), { kind: "local" });
  assert.deepEqual(parseConnectionSetting("studio"), { kind: "saved", name: "studio" });
});

test("saved connections resolve to a remote endpoint that is never started", async () => {
  const dir = home({ last: null, connections: [{ name: "studio", url: "https://studio.example.com/", token: "device-token", device: "d1", added: "2026-10-26T00:00:00Z" }, { name: "broken" }] });
  assert.deepEqual(readConnections(dir).map((c) => c.name), ["studio"], "entries without a url or token are skipped");
  const saved = new SavedEndpoints("studio", () => dir);
  assert.equal(saved.canStart, false);
  assert.deepEqual(await saved.resolve(), { url: "https://studio.example.com", token: "device-token", source: "remote" });
  await assert.rejects(new SavedEndpoints("laptop", () => dir).resolve(), /no saved connection "laptop"; there are: studio/);
  await assert.rejects(new SavedEndpoints("laptop", () => home()).resolve(), /malac remote add laptop/);
  assert.equal(new LocalEndpoints(() => dir, { start: async () => ({ url: "", token: "" }) }).canStart, true);
});
