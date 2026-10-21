import { spawnSync } from "node:child_process";
import { rustEnvironment } from "./toolchain.mjs";
const result = spawnSync("cargo", process.argv.slice(2), {
  env: rustEnvironment(),
  stdio: "inherit",
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
