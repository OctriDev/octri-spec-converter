/** Independent validation of the launch corpus; no source URI may be fetched. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { after, test } from "node:test";
import SwaggerParser from "@apidevtools/swagger-parser";
import { launchCases } from "./launch-cases.mjs";

function pointer(document, path) {
  assert.ok(path.startsWith("/"), `Expected JSON Pointer: ${path}`);
  return path
    .slice(1)
    .split("/")
    .reduce((value, token) => {
      const key = token.replaceAll("~1", "/").replaceAll("~0", "~");
      return value !== null &&
        typeof value === "object" &&
        Object.hasOwn(value, key)
        ? value[key]
        : undefined;
    }, document);
}

function convert(request) {
  const result = spawnSync("target/release/octri-spec-convert", [], {
    input: JSON.stringify({ abiVersion: 1, action: "convert", request }),
    encoding: "utf8",
    timeout: 30_000,
    maxBuffer: 128 * 1024 * 1024,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
  // These fixtures use JSON numbers representable in binary64. The separate
  // exact-number suites qualify large integers and decimal-token fidelity.
  const resultEnvelope = JSON.parse(result.stdout);
  assert.ok(resultEnvelope.Ok, result.stdout);
  return resultEnvelope.Ok;
}

const cases = launchCases();
const outcomes = [];
for (const adapter of ["swagger", "postman"]) {
  assert.ok(cases.filter((entry) => entry.adapter === adapter).length >= 20);
}
assert.equal(
  new Set(cases.map(({ adapter, fixture }) => `${adapter}/${fixture.name}`))
    .size,
  cases.length,
);

for (const { adapter, fixture, request } of cases) {
  test(`${adapter}: ${fixture.name}`, async () => {
    const report = convert(request);
    assert.equal(report.valid, true, JSON.stringify(report.diagnostics));
    assert.notEqual(report.status, "blocked", JSON.stringify(report.decisions));
    assert.ok(
      Object.keys(fixture.expected).length > 0,
      "Missing semantic oracle",
    );
    for (const [path, expected] of Object.entries(fixture.expected)) {
      assert.deepEqual(pointer(report.document, path), expected, path);
    }
    for (const path of fixture.absent ?? []) {
      assert.equal(pointer(report.document, path), undefined, path);
    }
    for (const [path, expected] of Object.entries(
      fixture.expectedSingleAuth ?? {},
    )) {
      const alternatives = pointer(report.document, `${path}/security`);
      assert.equal(alternatives.length, 1, `${path}: auth alternatives`);
      const requirements = Object.entries(alternatives[0]);
      assert.equal(requirements.length, 1, `${path}: auth conjunction`);
      const [name, scopes] = requirements[0];
      assert.ok(
        Array.isArray(expected.scopes),
        "Missing expected operation scopes",
      );
      assert.deepEqual(scopes, expected.scopes);
      assert.deepEqual(
        report.document.components.securitySchemes[name],
        expected.scheme,
      );
    }
    for (const code of fixture.decisionCodes ?? []) {
      assert.ok(
        report.decisions.some((decision) => decision.code === code),
        code,
      );
    }
    for (const decision of report.decisions) {
      if (["omitted", "blocked"].includes(decision.classification)) {
        assert.ok(
          (fixture.allowedLosses ?? []).includes(decision.code),
          `Unexpected loss: ${decision.code} at ${decision.source.pointer}`,
        );
      }
    }
    const reviewed = convert({
      ...request,
      config: {
        ...request.config,
        acceptedDecisions: report.decisions
          .filter((decision) => decision.reviewRequired)
          .map((decision) => decision.id),
      },
    });
    assert.equal(reviewed.status, "complete");
    assert.equal(reviewed.handoffAllowed, true);
    assert.deepEqual(reviewed.document, report.document);
    await SwaggerParser.validate(structuredClone(reviewed.document), {
      resolve: { external: false },
      dereference: { circular: "ignore" },
    });
    outcomes.push({
      adapter,
      name: fixture.name,
      outputSha256: reviewed.output.sha256,
    });
  });
}

after(async () => {
  const validator = JSON.parse(
    await readFile(
      new URL(
        "../node_modules/@apidevtools/swagger-parser/package.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const corpusSha256 = {};
  for (const adapter of ["swagger", "postman"]) {
    const bytes = await readFile(
      new URL(`../fixtures/launch/${adapter}.json`, import.meta.url),
    );
    corpusSha256[adapter] = createHash("sha256").update(bytes).digest("hex");
  }
  await mkdir("artifacts/qualification", { recursive: true });
  await writeFile(
    "artifacts/qualification/launch-corpus.json",
    JSON.stringify(
      {
        qualified: outcomes.length === cases.length,
        expectedCases: cases.length,
        validator: { name: validator.name, version: validator.version },
        nativeSha256: createHash("sha256")
          .update(await readFile("target/release/octri-spec-convert"))
          .digest("hex"),
        corpusSha256,
        outcomes,
      },
      null,
      2,
    ) + "\n",
  );
});
