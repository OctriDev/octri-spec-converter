import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { delimiter, join, resolve } from "node:path";
import { rustEnvironment } from "./toolchain.mjs";

if (!["darwin", "linux"].includes(process.platform))
  throw new Error("Native bindings are qualified on macOS/Linux only");
const run = (command, args, options = {}) =>
  execFileSync(command, args, { stdio: "inherit", ...options });
const output = resolve("artifacts/bindings");
const nativeDir = resolve("target/release");
mkdirSync(output, { recursive: true });
run(
  "cargo",
  [
    "build",
    "--release",
    "--locked",
    "-p",
    "octri-converter-ffi",
    "-p",
    "octri-converter-cli",
  ],
  { env: rustEnvironment() },
);
for (const target of [
  "bindings/go/octri_converter.h",
  "bindings/swift/Sources/COctriConverter/include/octri_converter.h",
]) {
  mkdirSync(resolve(target, ".."), { recursive: true });
  copyFileSync("bindings/c/include/octri_converter.h", target);
}
const suffix = process.platform === "darwin" ? "dylib" : "so";
const library = join(nativeDir, `liboctri_converter_ffi.${suffix}`);
if (process.platform === "darwin")
  run("install_name_tool", [
    "-id",
    `@rpath/liboctri_converter_ffi.${suffix}`,
    library,
  ]);
const homebrewJdk =
  "/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home";
const javaHome =
  process.env.JAVA_HOME ??
  (existsSync(homebrewJdk)
    ? homebrewJdk
    : process.platform === "darwin"
      ? execFileSync("/usr/libexec/java_home", ["-v", "11+"], {
          encoding: "utf8",
        }).trim()
      : "/usr/lib/jvm/default-java");
const javaEnv = {
  ...process.env,
  JAVA_HOME: javaHome,
  PATH: `${join(javaHome, "bin")}${delimiter}${process.env.PATH}`,
};
const jni = join(nativeDir, `liboctri_converter_jni.${suffix}`);
run("clang", [
  "-std=c11",
  "-Wall",
  "-Wextra",
  "-Werror",
  "-fPIC",
  "-shared",
  "-Ibindings/c/include",
  `-I${join(javaHome, "include")}`,
  `-I${join(javaHome, "include", process.platform === "darwin" ? "darwin" : "linux")}`,
  "bindings/java/native/bridge.c",
  `-L${nativeDir}`,
  "-loctri_converter_ffi",
  process.platform === "darwin"
    ? "-Wl,-rpath,@loader_path"
    : "-Wl,-rpath,$ORIGIN",
  "-o",
  jni,
]);
run(
  "mvn",
  ["-q", "-f", "bindings/java/pom.xml", "clean", "install", "-DskipTests"],
  { env: javaEnv },
);
run(
  "mvn",
  [
    "-q",
    "-f",
    "bindings/java/pom.xml",
    "dependency:build-classpath",
    `-Dmdep.outputFile=${join(output, "java-dependencies.txt")}`,
  ],
  { env: javaEnv },
);
const javaDependencies = readFileSync(
  join(output, "java-dependencies.txt"),
  "utf8",
).trim();
const javaJar = resolve("bindings/java/target/spec-converter-0.1.0.jar");
run(
  "mvn",
  ["-q", "-f", "bindings/kotlin/pom.xml", "clean", "package", "-DskipTests"],
  { env: javaEnv },
);
const kotlinJar = resolve(
  "bindings/kotlin/target/spec-converter-kotlin-0.1.0.jar",
);
const kotlinCheck = join(output, "kotlin-check.jar");
run(
  "kotlinc",
  [
    "bindings/kotlin/src/test/kotlin/BindingCheck.kt",
    "-jvm-target",
    "11",
    "-classpath",
    [javaJar, kotlinJar, javaDependencies].join(delimiter),
    "-include-runtime",
    "-d",
    kotlinCheck,
  ],
  { env: javaEnv },
);
const goCheck = join(output, "go-check");
run("go", ["build", "-o", goCheck, "./cmd/bindingcheck"], {
  cwd: resolve("bindings/go"),
  env: {
    ...process.env,
    CC: process.env.CC ?? "clang",
    CGO_ENABLED: "1",
    CGO_LDFLAGS: `-L${nativeDir} -Wl,-rpath,${nativeDir}`,
    GOTOOLCHAIN: "local",
  },
});
const swiftArgs = [
  "-Xlinker",
  `-L${nativeDir}`,
  "-Xlinker",
  "-rpath",
  "-Xlinker",
  nativeDir,
  ...(process.platform === "linux"
    ? ["-Xlinker", "-z", "-Xlinker", "separate-code"]
    : []),
];
run("swift", [
  "build",
  "-c",
  "release",
  "--package-path",
  "bindings/swift",
  "--product",
  "BindingCheck",
  ...swiftArgs,
]);
const swiftDirectory = execFileSync(
  "swift",
  [
    "build",
    "-c",
    "release",
    "--package-path",
    "bindings/swift",
    "--show-bin-path",
  ],
  { encoding: "utf8" },
).trim();
run("dart", ["pub", "get"], { cwd: resolve("bindings/dart") });
const dartCheck = join(output, "dart-check");
run("dart", [
  "compile",
  "exe",
  "bindings/dart/bin/check.dart",
  "-o",
  dartCheck,
]);
run("clang", [
  "-std=c11",
  "-Wall",
  "-Wextra",
  "-Werror",
  "-Ibindings/c/include",
  "bindings/c/check.c",
  `-L${nativeDir}`,
  "-loctri_converter_ffi",
  `-Wl,-rpath,${nativeDir}`,
  "-o",
  join(output, "c-check"),
]);
run(join(output, "c-check"), []);
writeFileSync(
  join(output, "build.json"),
  JSON.stringify(
    {
      library,
      jni,
      javaHome,
      javaJar,
      javaDependencies,
      kotlinJar,
      kotlinCheck,
      goCheck,
      swiftCheck: join(swiftDirectory, "BindingCheck"),
      dartCheck,
    },
    null,
    2,
  ) + "\n",
);
console.log("All native language consumers built. Run npm run test:bindings.");
