import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { runAction } from "./github-action.mjs";

test("Action preserves exact output, uses literal arguments, and fails incomplete review", async () => {
  const directory = await mkdtemp(join(tmpdir(), "octri-action-test-"));
  try {
    const name = "source;$(touch injected).json";
    const content =
      '{"openapi":"3.1.2","info":{"title":"Exact","version":"1"},"paths":{},"x-exact":0.100000000000000005}';
    await writeFile(join(directory, name), content);
    const env = {
      ...process.env,
      INPUT_INPUT: name,
      INPUT_ROOT: ".",
      INPUT_FORMAT: "json",
      INPUT_TARGET: "",
      INPUT_FILES: "",
      INPUT_CONFIG: "",
      INPUT_BINARY: resolve("target/release/octri-spec-convert"),
      GITHUB_WORKSPACE: directory,
      RUNNER_TEMP: directory,
      GITHUB_OUTPUT: join(directory, "outputs"),
    };
    const result = await runAction(env);
    assert.equal(result.exitCode, 0);
    assert.equal(result.status, "complete");
    assert.match(
      await readFile(result.document, "utf8"),
      /0\.100000000000000005/,
    );
    assert.equal(
      JSON.parse(await readFile(result.report, "utf8")).original.input.content,
      content,
    );
    assert.match(
      await readFile(env.GITHUB_OUTPUT, "utf8"),
      /handoff-allowed<</,
    );
    await assert.rejects(access(join(directory, "injected")));
    await writeFile(
      join(directory, "request.curl"),
      "curl https://example.com/pets",
    );
    const pending = await runAction({ ...env, INPUT_INPUT: "request.curl" });
    assert.equal(pending.status, "needs-review");
    assert.equal(pending.handoff, false);
    assert.equal(pending.exitCode, 1);
    await writeFile(join(directory, "invalid.json"), "{ invalid");
    assert.equal(
      (await runAction({ ...env, INPUT_INPUT: "invalid.json" })).exitCode,
      2,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
