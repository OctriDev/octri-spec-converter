/** Install packed packages outside this checkout; exercise shipped entry points. */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const directory = mkdtempSync(join(tmpdir(), "octri-converter-package-"));
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
try {
  execFileSync(process.execPath, ["scripts/package-node-native.mjs"], {
    stdio: "inherit",
  });
  const nativeOutput = resolve(
    "artifacts/npm-native",
    `${process.platform}-${process.arch}`,
  );
  const nativeManifest = JSON.parse(
    readFileSync(join(nativeOutput, "manifest.json"), "utf8"),
  );
  const archives = ["node", "browser"].map((name) => {
    const result = JSON.parse(
      execFileSync(
        npm,
        [
          "pack",
          resolve(`packages/${name}`),
          "--ignore-scripts",
          "--json",
          "--pack-destination",
          directory,
        ],
        { encoding: "utf8" },
      ),
    )[0];
    for (const path of [
      "numbers.mjs",
      "LICENSE",
      "OAI-LICENSE",
      "THIRD-PARTY-NOTICES.md",
    ])
      assert.ok(
        result.files.some((file) => file.path === path),
        `${name}: missing ${path}`,
      );
    assert(result.files.some((file) => file.path === "README.md"));
    return join(directory, result.filename);
  });
  archives.push(join(nativeOutput, nativeManifest.packages[0].filename));
  writeFileSync(
    join(directory, "package.json"),
    JSON.stringify({
      name: "converter-consumer-check",
      private: true,
      type: "module",
    }),
  );
  execFileSync(
    npm,
    ["install", "--ignore-scripts", "--no-audit", "--no-fund", ...archives],
    { cwd: directory, stdio: "pipe" },
  );
  execFileSync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      `
    import assert from 'node:assert/strict';
    import { Converter, LosslessNumber, object } from '@octri/spec-converter-native';
    import { createRequire } from 'node:module';
    const source = object({openapi:'3.1.2',info:{title:'Installed consumer',version:'1'},paths:{},'x-exact':new LosslessNumber('0.100000000000000005')});
    const report = await Converter.convert({input:source});
    assert.equal(report.document['x-exact'].token,'0.100000000000000005');
    const mapping = await Converter.export(report,'manifest');
    assert.ok(Array.isArray(mapping.mappings));
    const common = createRequire(import.meta.url)('@octri/spec-converter-native');
    assert.equal((await common.formats()).targets.length,11);
    const browser = await import('@octri/spec-converter-wasm');
    const worker = await import('@octri/spec-converter-wasm/worker');
    assert.equal(typeof worker.createWorkerClient,'function');
    assert.equal(browser.stringifyJson(browser.parseJson('0.100000000000000005')),'0.100000000000000005');
  `,
    ],
    { cwd: directory, stdio: "inherit" },
  );
  writeFileSync(
    join(directory, "consumer.ts"),
    `import { Converter, LosslessNumber, object, type ConversionReport } from '@octri/spec-converter-native';\nimport { createWorkerClient } from '@octri/spec-converter-wasm/worker';\nconst input = object({openapi:'3.1.2',info:{title:'Types',version:'1'},paths:{},'x-number':new LosslessNumber('1e400')});\nconst result: Promise<ConversionReport> = Converter.convert({input});\nconst worker = createWorkerClient(); worker.dispose(); void result;\n`,
  );
  execFileSync(
    process.execPath,
    [
      resolve("node_modules/typescript/bin/tsc"),
      "--strict",
      "--noEmit",
      "--target",
      "ES2022",
      "--module",
      "NodeNext",
      "--moduleResolution",
      "NodeNext",
      "consumer.ts",
    ],
    { cwd: directory, stdio: "inherit" },
  );
  const installed = JSON.parse(
    readFileSync(
      join(directory, "node_modules/@octri/spec-converter-native/package.json"),
      "utf8",
    ),
  );
  assert.notEqual(installed.private, true);
  assert.equal(installed.publishConfig.access, "public");
  assert.equal(
    installed.optionalDependencies[
      `@octri/spec-converter-native-${process.platform}-${process.arch}`
    ],
    installed.version,
  );
  assert.ok(installed.keywords.includes("openapi"));
  assert.ok(
    readFileSync(
      join(directory, "node_modules/@octri/spec-converter-native/README.md"),
      "utf8",
    ).includes("npm install"),
  );
  console.log(
    "Installed tarballs: Node ESM/CommonJS runtime, exact decimals, mapping manifests, browser/worker exports and TypeScript passed",
  );
} finally {
  rmSync(directory, { recursive: true, force: true });
}
