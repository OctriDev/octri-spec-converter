import { execFileSync } from "node:child_process";
import { copyFileSync, appendFileSync } from "node:fs";
import { rustEnvironment } from "./toolchain.mjs";

const env = rustEnvironment();
env.RUSTFLAGS = [env.RUSTFLAGS, "-C link-arg=--max-memory=536870912"]
  .filter(Boolean)
  .join(" ");
execFileSync(
  "cargo",
  [
    "build",
    "--release",
    "--locked",
    "-p",
    "octri-converter-wasm",
    "--target",
    "wasm32-unknown-unknown",
  ],
  { stdio: "inherit", env },
);
execFileSync(
  "wasm-bindgen",
  [
    "target/wasm32-unknown-unknown/release/octri_converter_wasm.wasm",
    "--target",
    "web",
    "--out-dir",
    "packages/browser/pkg",
    "--out-name",
    "octri_converter",
  ],
  { stdio: "inherit", env },
);
copyFileSync("packages/shared/client.mjs", "packages/browser/client.mjs");
copyFileSync("packages/shared/numbers.mjs", "packages/browser/numbers.mjs");
copyFileSync("packages/shared/json.mjs", "packages/browser/json.mjs");
copyFileSync("packages/shared/index.d.ts", "packages/browser/index.d.ts");
appendFileSync(
  "packages/browser/index.d.ts",
  "\nexport function initialize(options?: unknown): Promise<void>;\n",
);
