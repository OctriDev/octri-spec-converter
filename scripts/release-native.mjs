/** Build this platform's native libraries for the language packages. No upload. */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { buildJniBridge, findJavaHome, libraryFile } from "./native-build.mjs";
import { rustEnvironment } from "./toolchain.mjs";

const platform = `${process.platform}-${process.arch}`;
assert(
  ["darwin-arm64", "linux-x64", "linux-arm64"].includes(platform),
  `Unsupported native target: ${platform}`,
);
const run = (command, args) =>
  execFileSync(command, args, { stdio: "inherit", env: rustEnvironment() });
const output = resolve("artifacts/native", platform);
rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });

run("cargo", ["build", "--release", "--locked", "-p", "octri-converter-ffi"]);
const built = resolve("target/release");
const shared = join(output, libraryFile("octri_converter_ffi"));
const archive = join(output, libraryFile("octri_converter_ffi", "static"));
copyFileSync(join(built, libraryFile("octri_converter_ffi")), shared);
copyFileSync(
  join(built, libraryFile("octri_converter_ffi", "static")),
  archive,
);
// Hosts load the library from wherever their package manager put it.
if (process.platform === "darwin")
  run("install_name_tool", [
    "-id",
    `@rpath/${libraryFile("octri_converter_ffi")}`,
    shared,
  ]);
// Go links the archive. Drop std's debug info and the LLVM bitcode that LTO
// leaves in each object: two thirds of its size, unused by a normal link.
// Apple's strip corrupts the archive's assembly objects; LLVM objcopy does not.
const rustc = (args) =>
  execFileSync("rustc", args, { encoding: "utf8", env: rustEnvironment() });
const host = rustc(["-vV"]).match(/^host: (.+)$/m)[1];
const bitcode =
  process.platform === "darwin"
    ? ["__LLVM,__bitcode", "__LLVM,__cmdline"]
    : [".llvmbc", ".llvmcmd"];
run(
  join(
    rustc(["--print", "sysroot"]).trim(),
    "lib/rustlib",
    host,
    "bin/rust-objcopy",
  ),
  [
    "--strip-debug",
    ...bitcode.map((section) => `--remove-section=${section}`),
    archive,
  ],
);

const jni = join(output, libraryFile("octri_converter_jni"));
buildJniBridge(findJavaHome(), output, jni);
console.log(`Native libraries for ${platform}: ${output}`);
