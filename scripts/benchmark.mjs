/** Reproducible offline fixture; measure complete conversion and inventory search. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { cpus } from "node:os";
import { performance } from "node:perf_hooks";
import { chromium } from "playwright";
import { serve } from "../web/serve.mjs";

const samples = 20;
const paths = {};
for (let index = 0; index < 500; index++) {
  paths[`/resources/${index}`] = {
    get: {
      operationId: `getResource${index}`,
      summary: `Read resource ${index}`,
      description: "Representative operation documentation. ".repeat(40),
      parameters: [
        { name: "expand", in: "query", schema: { type: "boolean" } },
      ],
      responses: {
        200: {
          description: "Resource",
          content: {
            "application/json": {
              schema: {
                type: "object",
                properties: {
                  id: { type: "integer" },
                  name: { type: "string" },
                },
              },
            },
          },
        },
      },
    },
  };
}
const source = {
  openapi: "3.1.2",
  info: { title: "Qualification fixture", version: "1" },
  paths,
  "x-padding": "",
};
const padding = 1024 * 1024 - Buffer.byteLength(JSON.stringify(source));
assert.ok(padding >= 0, "Fixture construction exceeds 1 MiB");
source["x-padding"] = "x".repeat(padding);
const content = JSON.stringify(source);
assert.equal(Buffer.byteLength(content), 1024 * 1024);
const request = {
  input: { uri: "memory://benchmark/api.json", content, generated: false },
  files: {},
  config: {},
};
const envelope = JSON.stringify({ abiVersion: 1, action: "convert", request });
function summary(values) {
  const sorted = [...values].sort((left, right) => left - right);
  return {
    samples: values,
    p50Ms: sorted[Math.ceil(sorted.length * 0.5) - 1],
    p95Ms: sorted[Math.ceil(sorted.length * 0.95) - 1],
    maxMs: sorted.at(-1),
  };
}
function native() {
  const started = performance.now();
  const result = spawnSync("target/release/octri-spec-convert", [], {
    input: envelope,
    encoding: "utf8",
    maxBuffer: 128 * 1024 * 1024,
    timeout: 30_000,
  });
  const elapsed = performance.now() - started;
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout).Ok;
  assert.equal(report?.valid, true);
  assert.equal(report?.handoffAllowed, true);
  assert.equal(report?.inspection?.statistics?.operations, 500);
  return elapsed;
}
const coldNativeMs = native();
const nativeSamples = Array.from({ length: samples }, native);
const { server, origin } = await serve();
const browser = await chromium.launch();
let browserResult;
try {
  const page = await browser.newPage();
  await page.goto(origin + "/web/index.html");
  browserResult = await page.evaluate(
    async ({ envelope, samples }) => {
      const { createWorkerClient } = await import(
        "/packages/browser/worker-client.mjs"
      );
      const { renderInspection } = await import("/web/inspection.mjs");
      const client = createWorkerClient();
      const conversion = [];
      const search = [];
      let report;
      try {
        for (let index = -1; index < samples; index++) {
          const start = performance.now();
          const response = JSON.parse(await client.executeJson(envelope));
          const elapsed = performance.now() - start;
          if (
            !response.Ok?.handoffAllowed ||
            response.Ok.inspection?.statistics?.operations !== 500
          )
            throw new Error(
              "Benchmark conversion did not produce complete inventory",
            );
          report = response.Ok;
          if (index === -1) conversion.coldMs = elapsed;
          else conversion.push(elapsed);
        }
        const parent = document.createElement("section");
        document.body.replaceChildren(parent);
        renderInspection(parent, report, { output() {}, navigate() {} });
        const input = parent.querySelector('input[type="search"]');
        for (let index = 0; index < samples; index++) {
          input.value = index % 2 ? "resource" : "getResource499";
          const start = performance.now();
          input.dispatchEvent(new Event("input"));
          // Include layout work, not merely event dispatch or array filtering.
          parent.getBoundingClientRect();
          search.push(performance.now() - start);
        }
        return { coldMs: conversion.coldMs, conversion, search };
      } finally {
        client.dispose();
      }
    },
    { envelope, samples },
  );
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const evidence = {
  measuredAt: new Date().toISOString(),
  reference: {
    platform: process.platform,
    architecture: process.arch,
    cpu: cpus()[0]?.model,
    node: process.version,
    browser: browser.version(),
  },
  fixture: {
    bytes: Buffer.byteLength(content),
    operations: 500,
    sha256: hash(content),
  },
  artifacts: {
    nativeSha256: hash(await readFile("target/release/octri-spec-convert")),
    wasmSha256: hash(
      await readFile("packages/browser/pkg/octri_converter_bg.wasm"),
    ),
  },
  measurements: {
    native: { coldMs: coldNativeMs, ...summary(nativeSamples) },
    browserWorker: {
      coldMs: browserResult.coldMs,
      ...summary(browserResult.conversion),
    },
    inventorySearch: summary(browserResult.search),
  },
  scope:
    "Native includes process startup and ABI serialization. Browser includes worker round trip and decoding; excludes network after the cold sample. Search includes layout. No application rendering is included in conversion timing.",
};
evidence.passed =
  evidence.measurements.native.p95Ms < 3000 &&
  evidence.measurements.browserWorker.p95Ms < 3000 &&
  evidence.measurements.inventorySearch.p95Ms < 150;
await mkdir("artifacts/qualification", { recursive: true });
await writeFile(
  "artifacts/qualification/performance.json",
  JSON.stringify(evidence, null, 2) + "\n",
);
console.log(
  JSON.stringify({
    passed: evidence.passed,
    nativeP95Ms: evidence.measurements.native.p95Ms,
    browserP95Ms: evidence.measurements.browserWorker.p95Ms,
    searchP95Ms: evidence.measurements.inventorySearch.p95Ms,
  }),
);
if (!evidence.passed) process.exitCode = 1;
