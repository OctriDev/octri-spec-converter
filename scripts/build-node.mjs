import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import { rustEnvironment } from "./toolchain.mjs";

execFileSync(
  "cargo",
  ["build", "--release", "--locked", "-p", "octri-converter-node"],
  { stdio: "inherit", env: rustEnvironment() },
);
const name =
  process.platform === "darwin"
    ? "liboctri_converter_node.dylib"
    : process.platform === "win32"
      ? "octri_converter_node.dll"
      : "liboctri_converter_node.so";
const staging = mkdtempSync("packages/node/.native-build-");
try {
  const addon = join(staging, "octri-converter.node");
  copyFileSync(`target/release/${name}`, addon);
  renameSync(addon, "packages/node/octri-converter.node");
} finally {
  rmSync(staging, { recursive: true, force: true });
}
for (const folder of ["node"]) {
  copyFileSync("packages/shared/client.mjs", `packages/${folder}/client.mjs`);
  copyFileSync("packages/shared/numbers.mjs", `packages/${folder}/numbers.mjs`);
  copyFileSync("packages/shared/json.mjs", `packages/${folder}/json.mjs`);
  copyFileSync("packages/shared/index.d.ts", `packages/${folder}/index.d.ts`);
}
