import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { delimiter, join, resolve } from "node:path";
import {
  buildJniBridge,
  findJavaHome,
  libraryFile,
  version,
} from "./native-build.mjs";
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
copyFileSync(
  "bindings/c/include/octri_converter.h",
  "bindings/go/octri_converter.h",
);
const library = join(nativeDir, libraryFile("octri_converter_ffi"));
if (process.platform === "darwin")
  run("install_name_tool", [
    "-id",
    `@rpath/${libraryFile("octri_converter_ffi")}`,
    library,
  ]);
const javaHome = findJavaHome();
const javaEnv = {
  ...process.env,
  JAVA_HOME: javaHome,
  PATH: `${join(javaHome, "bin")}${delimiter}${process.env.PATH}`,
};
const jni = join(nativeDir, libraryFile("octri_converter_jni"));
buildJniBridge(javaHome, nativeDir, jni);
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
const javaJar = resolve(`bindings/java/target/spec-converter-${version}.jar`);
run(
  "mvn",
  ["-q", "-f", "bindings/kotlin/pom.xml", "clean", "package", "-DskipTests"],
  { env: javaEnv },
);
const kotlinJar = resolve(
  `bindings/kotlin/target/spec-converter-kotlin-${version}.jar`,
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
// Swift loads the C ABI at runtime, so only the Linux segment policy applies.
const swiftArgs =
  process.platform === "linux"
    ? ["-Xlinker", "-z", "-Xlinker", "separate-code"]
    : [];
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
