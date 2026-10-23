import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

/** The workspace version every binding package and crate is released under. */
export const version = readFileSync("Cargo.toml", "utf8").match(
  /\[workspace\.package\][^[]*?\nversion = "([^"]+)"/,
)[1];

/** Platform file name of a native library built from `name`. */
export const libraryFile = (name, kind = "shared") =>
  kind === "static"
    ? `lib${name}.a`
    : process.platform === "darwin"
      ? `lib${name}.dylib`
      : `lib${name}.so`;

export function findJavaHome() {
  const homebrewJdk =
    "/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home";
  return (
    process.env.JAVA_HOME ??
    (existsSync(homebrewJdk)
      ? homebrewJdk
      : process.platform === "darwin"
        ? execFileSync("/usr/libexec/java_home", ["-v", "11+"], {
            encoding: "utf8",
          }).trim()
        : "/usr/lib/jvm/default-java")
  );
}

/** Compile the JNI bridge against the C ABI library in `nativeDir`. */
export function buildJniBridge(javaHome, nativeDir, output) {
  const jniPlatform = process.platform === "darwin" ? "darwin" : "linux";
  execFileSync(
    "clang",
    [
      "-std=c11",
      "-Wall",
      "-Wextra",
      "-Werror",
      "-fPIC",
      "-shared",
      "-Ibindings/c/include",
      `-I${join(javaHome, "include")}`,
      `-I${join(javaHome, "include", jniPlatform)}`,
      "bindings/java/native/bridge.c",
      `-L${nativeDir}`,
      "-loctri_converter_ffi",
      process.platform === "darwin"
        ? "-Wl,-rpath,@loader_path"
        : "-Wl,-rpath,$ORIGIN",
      "-o",
      output,
    ],
    { stdio: "inherit" },
  );
}
