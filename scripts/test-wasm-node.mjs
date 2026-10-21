/** WASM engine parity under Node. This does not qualify browser or worker UI. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { initSync, execute } from "../packages/browser/pkg/octri_converter.js";
import { parseJson, stringifyJson } from "../packages/shared/client.mjs";
import { transportCases } from "./transport-cases.mjs";

initSync({
  module: readFileSync(
    new URL("../packages/browser/pkg/octri_converter_bg.wasm", import.meta.url),
  ),
});
const cases = transportCases();
for (const envelope of cases) {
  const input = stringifyJson(envelope);
  const expected = spawnSync("target/release/octri-spec-convert", ["execute"], {
    input,
    encoding: "utf8",
    timeout: 30_000,
    maxBuffer: 128 * 1024 * 1024,
  });
  assert.ok([0, 2].includes(expected.status), expected.stderr);
  assert.deepEqual(
    parseJson(execute(input)),
    parseJson(expected.stdout),
    envelope.action,
  );
}
console.log(
  `WASM engine under Node: ${cases.length} complete Rust results matched. Browser and worker acceptance require separate checks.`,
);
