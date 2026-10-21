import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, stat, mkdir, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { resolve, extname, sep } from "node:path";
import { chromium, firefox, webkit } from "playwright";
import { parseJson, stringifyJson } from "../packages/shared/client.mjs";
import { transportCases } from "./transport-cases.mjs";

const root = resolve(".");
const types = {
  ".mjs": "text/javascript",
  ".js": "text/javascript",
  ".wasm": "application/wasm",
  ".json": "application/json",
};
const server = createServer(async (request, response) => {
  response.setHeader(
    "Content-Security-Policy",
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'",
  );
  try {
    if (request.url === "/") {
      response.setHeader("Content-Type", "text/html");
      response.end(
        "<!doctype html><html lang=en><title>Converter browser qualification</title></html>",
      );
      return;
    }
    if (request.url === "/hanging-worker.mjs") {
      response.setHeader("Content-Type", "text/javascript");
      response.end("self.onmessage = () => {};\n");
      return;
    }
    const path = resolve(
      root,
      "." +
        decodeURIComponent(new URL(request.url, "http://localhost").pathname),
    );
    if (!path.startsWith(root + sep) || !(await stat(path)).isFile()) {
      response.writeHead(404).end();
      return;
    }
    response.setHeader(
      "Content-Type",
      types[extname(path)] ?? "application/octet-stream",
    );
    response.end(await readFile(path));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const envelopes = transportCases().map(stringifyJson);
const expected = envelopes.map((input) => {
  const result = spawnSync("target/release/octri-spec-convert", [], {
    input,
    encoding: "utf8",
    maxBuffer: 128 * 1024 * 1024,
    timeout: 30000,
  });
  assert.ok([0, 1, 2].includes(result.status), result.stderr);
  return parseJson(result.stdout);
});
const evidence = [];
const browserTypes = { chromium, firefox, webkit };
const selectedBrowser = process.env.OCTRI_BROWSER;
if (selectedBrowser && !Object.hasOwn(browserTypes, selectedBrowser))
  throw new Error(`Unsupported browser: ${selectedBrowser}`);
try {
  for (const [name, browserType] of Object.entries(browserTypes).filter(
    ([name]) => !selectedBrowser || name === selectedBrowser,
  )) {
    const browser = await browserType.launch();
    try {
      const page = await browser.newPage();
      const errors = [];
      const outbound = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("request", (request) => {
        if (new URL(request.url()).origin !== origin)
          outbound.push(request.url());
      });
      await page.goto(origin);
      const result = await page.evaluate(async (envelopes) => {
        const { createWorkerClient } = await import(
          "/packages/browser/worker-client.mjs"
        );
        const client = createWorkerClient();
        const responses = [];
        for (const envelope of envelopes)
          responses.push(await client.executeJson(envelope));
        const pending = client.formats().then(
          () => "resolved",
          (error) => error.code,
        );
        client.cancel();
        const cancelled = await pending;
        const restarted = (await client.formats()).engineVersion;
        client.dispose();
        const timeoutClient = createWorkerClient({
          timeoutMs: 20,
          workerUrl: new URL("/hanging-worker.mjs", location.href),
        });
        const timedOut = await timeoutClient.formats().then(
          () => "resolved",
          (error) => error.code,
        );
        timeoutClient.dispose();
        const { default: initialize } = await import(
          "/packages/browser/pkg/octri_converter.js"
        );
        const wasm = await initialize();
        let memoryCapped = false;
        try {
          wasm.memory.grow(8193 - wasm.memory.buffer.byteLength / 65536);
        } catch (error) {
          memoryCapped = error instanceof RangeError;
        }
        return { responses, cancelled, restarted, timedOut, memoryCapped };
      }, envelopes);
      assert.equal(result.responses.length, expected.length);
      result.responses.forEach((raw, index) =>
        assert.deepEqual(
          parseJson(raw),
          expected[index],
          `${name}: case ${index}`,
        ),
      );
      assert.equal(result.cancelled, "CANCELLED");
      assert.equal(result.timedOut, "WORKER_TIMEOUT");
      assert.equal(typeof result.restarted, "string");
      assert.equal(
        result.memoryCapped,
        true,
        `${name}: WASM memory must stay below 512 MiB`,
      );
      let failedDownload = false;
      await page.route("**/octri_converter_bg.wasm", (route) => {
        if (!failedDownload) {
          failedDownload = true;
          return route.fulfill({
            status: 503,
            body: "Temporary engine download failure",
          });
        }
        return route.continue();
      });
      const retry = await page.evaluate(async () => {
        const { createWorkerClient } = await import(
          "/packages/browser/worker-client.mjs"
        );
        const client = createWorkerClient();
        try {
          const first = await client.formats().then(
            () => "unexpected-success",
            (error) => error.code,
          );
          const version = (await client.formats()).engineVersion;
          return { first, version };
        } finally {
          client.dispose();
        }
      });
      await page.unroute("**/octri_converter_bg.wasm");
      assert.equal(retry.first, "WORKER_ENGINE");
      assert.equal(typeof retry.version, "string");
      assert.deepEqual(
        outbound,
        [],
        `${name}: source data triggered external requests`,
      );
      assert.deepEqual(errors, []);
      evidence.push({
        engine: name,
        version: browser.version(),
        cases: envelopes.length,
        cancellation: true,
        restart: true,
        watchdog: true,
        outboundRequests: outbound.length,
      });
      console.log(
        `${name}: ${envelopes.length} full Rust/WASM results, CSP, cancellation, restart and watchdog passed`,
      );
    } finally {
      await browser.close();
    }
  }
  await mkdir("artifacts/browser", { recursive: true });
  await writeFile(
    "artifacts/browser/engine.json",
    JSON.stringify(evidence, null, 2) + "\n",
  );
} finally {
  await new Promise((resolve) => server.close(resolve));
}
