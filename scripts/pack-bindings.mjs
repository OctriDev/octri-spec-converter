import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { basename, join, resolve } from "node:path";

const run = (command, args, options = {}) =>
  execFileSync(command, args, { stdio: "inherit", ...options });
const build = JSON.parse(readFileSync("artifacts/bindings/build.json", "utf8"));
const output = resolve(
  "artifacts/release",
  `${process.platform}-${process.arch}`,
  "bindings",
);
// Replace this generated release directory so stale dependency jars cannot leak
// into a newer manifest after a security upgrade.
rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });
for (const file of [
  build.library,
  build.jni,
  build.javaJar,
  build.kotlinJar,
  "bindings/c/include/octri_converter.h",
  "README.md",
  "LICENSE",
  "THIRD-PARTY-NOTICES.md",
  "vendor/octri-parser/schemas/OAI-LICENSE",
]) {
  const name = basename(file);
  copyFileSync(file, join(output, name));
}
for (const dependency of build.javaDependencies.split(
  process.platform === "win32" ? ";" : ":",
))
  copyFileSync(dependency, join(output, basename(dependency)));
// Kotlin's stdlib is a JVM runtime dependency, not part of the thin facade jar.
const home = process.env.HOME;
const kotlinRuntime = join(
  home,
  ".m2/repository/org/jetbrains/kotlin/kotlin-stdlib/2.3.21/kotlin-stdlib-2.3.21.jar",
);
copyFileSync(kotlinRuntime, join(output, basename(kotlinRuntime)));
const python = process.env.OCTRI_BUILD_PYTHON ?? "python3";
if (python !== "python3" && !existsSync(python))
  throw new Error(
    "Set OCTRI_BUILD_PYTHON to a Python interpreter with build installed",
  );
run(python, ["-m", "build", "--wheel", "--outdir", output, "bindings/python"]);
run(
  "gem",
  [
    "build",
    "octri-spec-converter.gemspec",
    "--output",
    join(output, "octri-spec-converter-0.1.0.gem"),
  ],
  { cwd: resolve("bindings/ruby") },
);
// Source archives contain only reviewed source/manifests, never tool caches or
// conformance outputs. The generated C header is included for standalone builds.
function sourceArchive(name, files, cwd = process.cwd()) {
  run(
    "tar",
    [
      "--exclude=._*",
      "--exclude=.DS_Store",
      "--exclude=__MACOSX",
      "-czf",
      join(output, `${name}-0.1.0.tar.gz`),
      ...files,
    ],
    {
      cwd,
      // BSD tar otherwise serializes macOS resource forks as AppleDouble
      // members, even when those files do not exist in the source directory.
      env: { ...process.env, COPYFILE_DISABLE: "1" },
    },
  );
}

for (const [name, files] of [
  ["go", ["go.mod", "converter.go", "octri_converter.h"]],
  ["swift", ["Package.swift", "Sources", "Tests"]],
  ["dart", ["pubspec.yaml", "lib"]],
  ["php", ["composer.json", "src"]],
]) {
  sourceArchive(
    name,
    [...files, "LICENSE", "THIRD-PARTY-NOTICES.md", "OAI-LICENSE"],
    resolve("bindings", name),
  );
}
sourceArchive("rust", [
  "Cargo.toml",
  "Cargo.lock",
  "rust-toolchain.toml",
  "LICENSE",
  "THIRD-PARTY-NOTICES.md",
  "crates",
  "vendor",
  "fixtures",
]);
const records = readdirSync(output)
  .filter((name) => name !== "manifest.json")
  .sort()
  .map((name) => {
    const bytes = readFileSync(join(output, name));
    return {
      name,
      bytes: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
    };
  });
writeFileSync(
  join(output, "manifest.json"),
  JSON.stringify(
    {
      version: "0.1.0",
      abiVersion: 1,
      platform: process.platform,
      architecture: process.arch,
      distribution:
        "local release candidates; no registry publication performed",
      files: records,
    },
    null,
    2,
  ) + "\n",
);
console.log(`Eight-language plus Rust release set: ${output}`);
