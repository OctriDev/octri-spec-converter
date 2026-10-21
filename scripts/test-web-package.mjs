import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(".");
const output = resolve("artifacts/web");
const temp = mkdtempSync(join(tmpdir(), "octri-spec-converter-web-"));
try {
  execFileSync(process.execPath, ["scripts/package-web.mjs"], {
    cwd: root,
    stdio: "inherit",
  });
  const packed = execFileSync("npm", ["pack", output, "--json"], {
    cwd: temp,
    encoding: "utf8",
  });
  const [{ filename, files }] = JSON.parse(packed);
  const packedPaths = new Set(files.map(({ path }) => path));
  assert(
    packedPaths.has("README.md"),
    "npm package must include usage documentation",
  );
  const metadata = JSON.parse(
    readFileSync(join(output, "package.json"), "utf8"),
  );
  assert(
    metadata.keywords?.length >= 3,
    "npm package must include searchable keywords",
  );
  const manifest = JSON.parse(
    readFileSync(join(output, "manifest.json"), "utf8"),
  );
  assert.equal(
    JSON.parse(readFileSync(join(output, "package.json"))).name,
    "@octri/spec-converter-web",
  );
  assert(packedPaths.has("web/index.html"));
  for (const path of Object.keys(manifest.files))
    assert(packedPaths.has(path), `npm package omitted ${path}`);
  execFileSync("npm", ["install", "--ignore-scripts", join(temp, filename)], {
    cwd: temp,
    stdio: "inherit",
  });
  console.log(`Installed ${filename} outside source checkout`);
} finally {
  rmSync(temp, { recursive: true, force: true });
}
