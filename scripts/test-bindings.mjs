import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { delimiter, join, resolve } from "node:path";
import { parseJson, stringifyJson } from "../packages/shared/client.mjs";
import { LosslessNumber, numberIdentity } from "../packages/shared/numbers.mjs";
import { transportCases } from "./transport-cases.mjs";

const build = JSON.parse(readFileSync("artifacts/bindings/build.json", "utf8"));
const env = { ...process.env, PYTHONPATH: resolve("bindings/python/src") };
const java = join(build.javaHome, "bin/java");
const targets = [
  ["python", "python3", ["bindings/python/check.py", build.library]],
  ["go", build.goCheck, []],
  [
    "java",
    java,
    [
      "-cp",
      [
        resolve("bindings/java/target/test-classes"),
        build.javaJar,
        build.javaDependencies,
      ].join(delimiter),
      "BindingCheck",
      build.library,
      build.jni,
    ],
  ],
  [
    "kotlin",
    java,
    [
      "-cp",
      [
        build.kotlinCheck,
        build.kotlinJar,
        build.javaJar,
        build.javaDependencies,
      ].join(delimiter),
      "BindingCheckKt",
      build.library,
      build.jni,
    ],
  ],
  [
    "php",
    "php",
    ["-d", "ffi.enable=true", "bindings/php/check.php", build.library, "--raw"],
  ],
  ["ruby", "ruby", ["bindings/ruby/check.rb", build.library]],
  ["swift", build.swiftCheck, []],
  ["dart", build.dartCheck, [build.library, "--raw"]],
];
const rust = (envelope) => {
  const result = spawnSync("target/release/octri-spec-convert", [], {
    input: stringifyJson(envelope),
    encoding: "utf8",
    timeout: 30000,
    maxBuffer: 128 * 1024 * 1024,
  });
  assert.ok(
    [0, 1, 2].includes(result.status),
    `Rust transport failed: ${result.stderr}`,
  );
  return parseJson(result.stdout);
};
const envelopes = transportCases();
const report = rust(envelopes[0]).Ok;
assert.ok(report);
envelopes.push(
  ...["json", "html", "manifest", "artifact"].map((format) => ({
    abiVersion: 1,
    action: "export",
    report,
    format,
  })),
);
envelopes.push({
  abiVersion: 1,
  action: "compare",
  before: report,
  after: report,
});
const tampered = structuredClone(report);
tampered.handoffAllowed = !tampered.handoffAllowed;
envelopes.push({
  abiVersion: 1,
  action: "export",
  report: tampered,
  format: "artifact",
});
const references = envelopes.map(rust);
// Host decimal libraries may choose equivalent exponent spellings. Compare
// mathematical tokens while keeping precision-sensitive differences visible.
function comparable(value) {
  if (value instanceof LosslessNumber)
    return { exactNumber: numberIdentity(value.token) };
  if (Array.isArray(value)) return value.map(comparable);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, comparable(item)]),
    );
  return value;
}
let checks = 0;
for (const [language, command, args] of targets) {
  for (const [index, envelope] of envelopes.entries()) {
    const expected = references[index];
    let actual;
    try {
      actual = parseJson(
        execFileSync(command, args, {
          input: stringifyJson(envelope),
          encoding: "utf8",
          env: { ...env, OCTRI_BINDING_RAW: expected.Err ? "1" : "0" },
          timeout: 30000,
          maxBuffer: 128 * 1024 * 1024,
        }),
      );
    } catch (cause) {
      throw new Error(
        `${language}: ${envelope.action}, case ${index}, source ${envelope.request?.input?.uri ?? "transport"}`,
        { cause },
      );
    }
    if (expected.Err?.code === "INVALID_REQUEST") {
      const semantic = (result) => ({
        ...result.Err,
        message: result.Err.message.replace(/ at line \d+ column \d+$/, ""),
      });
      assert.deepEqual(
        semantic(actual),
        semantic(expected),
        `${language}: case ${index}`,
      );
    } else
      assert.deepEqual(
        comparable(actual),
        comparable(expected),
        `${language}: ${envelope.action}, case ${index}`,
      );
    checks++;
  }
  console.log(
    `${language}: ${envelopes.length} full results match Rust, including exact u64 and Unicode keys`,
  );
}
writeFileSync(
  "artifacts/bindings/parity.json",
  JSON.stringify(
    {
      checks,
      languages: targets.map(([name]) => name),
      cases: envelopes.length,
    },
    null,
    2,
  ) + "\n",
);

// PHP and Dart expose guarded decoded APIs alongside their lossless raw APIs.
// Exercise both contracts; raw parity alone cannot detect an unsafe facade.
const safe = {
  input: {
    uri: "memory://facade/api.json",
    content:
      '{"openapi":"3.1.2","info":{"title":"Facade","version":"1"},"paths":{}}',
  },
};
for (const [language, command, rawArgs] of targets.filter(([name]) =>
  ["php", "dart"].includes(name),
)) {
  const args = rawArgs.filter((argument) => argument !== "--raw");
  for (const envelope of [
    { abiVersion: 1, action: "formats" },
    { abiVersion: 1, action: "detect", request: safe },
    { abiVersion: 1, action: "batch", requests: [safe] },
  ]) {
    const actual = parseJson(
      execFileSync(command, args, {
        input: stringifyJson(envelope),
        encoding: "utf8",
        env,
        timeout: 30000,
        maxBuffer: 128 * 1024 * 1024,
      }),
    );
    assert.deepEqual(
      actual,
      rust(envelope),
      `${language}: typed ${envelope.action}`,
    );
  }
  for (const number of ["0.100000000000000005", "1e400"]) {
    const request = {
      input: {
        ...safe.input,
        content: safe.input.content.replace(
          '"paths":{}',
          `"paths":{},"x-exact":${number}`,
        ),
      },
    };
    const result = spawnSync(command, args, {
      input: stringifyJson({ abiVersion: 1, action: "convert", request }),
      encoding: "utf8",
      env,
      timeout: 30000,
    });
    assert.notEqual(result.status, 0, `${language}: decoded precision guard`);
    assert.match(result.stderr, /exact JSON number|lossless|Lossless/i);
  }
  console.log(`${language}: decoded detect/batch and precision guards passed`);
}
