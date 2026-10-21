/** GitHub adapter owns files/processes; conversion and review remain in Rust. */
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { appendFile, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const actionRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
async function output(path, values) {
  if (!path) return;
  for (const [name, value] of Object.entries(values)) {
    const delimiter = `octri_${randomUUID()}`;
    await appendFile(
      path,
      `${name}<<${delimiter}\n${String(value)}\n${delimiter}\n`,
    );
  }
}

export async function runAction(env = process.env) {
  const input = env.INPUT_INPUT;
  if (!input) throw new Error("input is required");
  const format = env.INPUT_FORMAT || "json";
  if (!["json", "yaml"].includes(format))
    throw new Error("format must be json or yaml");
  const workspace = resolve(env.GITHUB_WORKSPACE || ".");
  const root = resolve(workspace, env.INPUT_ROOT || ".");
  const directory = await mkdtemp(
    join(env.RUNNER_TEMP || tmpdir(), "octri-conversion-"),
  );
  const document = join(directory, `openapi.${format}`);
  const report = join(directory, "conversion-report.json");
  const diagnostics = join(directory, "diagnostics.txt");
  let binary = env.INPUT_BINARY
    ? resolve(workspace, env.INPUT_BINARY)
    : undefined;
  if (!binary) {
    // Build only this trusted action checkout. Customer roots and configuration
    // never become build scripts, shell expressions or Cargo manifests.
    const target = join(directory, "target");
    for (const args of [
      ["toolchain", "install", "1.95.0", "--profile", "minimal"],
      [
        "run",
        "1.95.0",
        "cargo",
        "build",
        "--manifest-path",
        join(actionRoot, "Cargo.toml"),
        "--target-dir",
        target,
        "--release",
        "--locked",
        "-p",
        "octri-converter-cli",
      ],
    ]) {
      const built = spawnSync("rustup", args, {
        cwd: actionRoot,
        env,
        stdio: "inherit",
        timeout: 15 * 60 * 1000,
      });
      if (built.error || built.status !== 0)
        throw new Error("Pinned Rust converter build failed");
    }
    binary = join(
      target,
      "release",
      process.platform === "win32"
        ? "octri-spec-convert.exe"
        : "octri-spec-convert",
    );
  }
  const args = [
    "convert",
    "--root-dir",
    root,
    "--format",
    format,
    "--out",
    document,
    "--report",
    report,
  ];
  if (env.INPUT_TARGET) args.push("--target", env.INPUT_TARGET);
  if (env.INPUT_CONFIG)
    args.push("--config", resolve(workspace, env.INPUT_CONFIG));
  const files = (env.INPUT_FILES || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (files.length > 100)
    throw new Error("At most 100 additional source patterns are allowed");
  for (const file of files) args.push("--file", file);
  args.push("--", input);
  const result = spawnSync(binary, args, {
    cwd: workspace,
    env,
    encoding: "utf8",
    stdio: ["ignore", "ignore", "pipe"],
    maxBuffer: 1024 * 1024,
    timeout: 35_000,
  });
  // Never send source-derived diagnostics or full reports to workflow logs.
  // Users can explicitly inspect or retain these private runner files.
  await writeFile(diagnostics, result.stderr || "");
  let status = "error";
  let handoff = false;
  try {
    // Display-only projection. Rust's original document/report files remain
    // untouched, including decimal tokens beyond JavaScript Number precision.
    const parsed = JSON.parse(await readFile(report, "utf8"));
    if (["complete", "needs-review", "blocked"].includes(parsed.status))
      status = parsed.status;
    handoff = parsed.handoffAllowed === true && status === "complete";
  } catch {
    /* CLI failures can occur before a report exists. */
  }
  await output(env.GITHUB_OUTPUT, {
    document,
    report,
    diagnostics,
    status,
    "handoff-allowed": handoff,
  });
  const exitCode = result.error || result.status === null ? 2 : result.status;
  console.log(
    `Spec Converter: ${status}; handoff ${handoff ? "allowed" : "blocked"}. Evidence remains in runner temporary files.`,
  );
  return {
    exitCode: handoff ? exitCode : Math.max(1, exitCode),
    document,
    report,
    diagnostics,
    status,
    handoff,
  };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    process.exitCode = (await runAction()).exitCode;
  } catch (error) {
    console.error(
      error instanceof Error ? error.message : "Spec Converter action failed",
    );
    process.exitCode = 2;
  }
}
