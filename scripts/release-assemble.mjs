/**
 * Assemble the registry packages from every platform's native libraries.
 *
 *   node scripts/release-assemble.mjs [python|ruby|maven|dart|go|php|swift|c ...]
 *
 * Reads artifacts/native/<platform>/ (scripts/release-native.mjs on each
 * platform) and writes artifacts/packages/<language>/. Platform wheels and gems
 * carry one library; Maven, Dart and the mirror trees carry all three. No upload.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { version } from "./native-build.mjs";

const PLATFORMS = {
  "darwin-arm64": {
    shared: "liboctri_converter_ffi.dylib",
    jni: "liboctri_converter_jni.dylib",
    wheel: "macosx_11_0_arm64",
    gem: "arm64-darwin",
    go: "darwin_arm64",
  },
  // Debian 12 (glibc 2.36) is the Linux build baseline; see release.yml.
  "linux-x64": {
    shared: "liboctri_converter_ffi.so",
    jni: "liboctri_converter_jni.so",
    wheel: "manylinux_2_36_x86_64",
    gem: "x86_64-linux",
    go: "linux_amd64",
  },
  "linux-arm64": {
    shared: "liboctri_converter_ffi.so",
    jni: "liboctri_converter_jni.so",
    wheel: "manylinux_2_36_aarch64",
    gem: "aarch64-linux",
    go: "linux_arm64",
  },
};
const NOTICES = [
  "LICENSE",
  "THIRD-PARTY-NOTICES.md",
  "vendor/octri-parser/schemas/OAI-LICENSE",
];
// Build output and the repository's own check consumers stay out of packages.
const DEVELOPMENT = new Set([
  "target",
  "build",
  "dist",
  ".build",
  ".dart_tool",
  "pubspec.lock",
  "__pycache__",
  "check.py",
  "check.rb",
  "check.php",
  "bin",
]);

const native = (platform, file) => resolve("artifacts/native", platform, file);
const output = resolve("artifacts/packages");
const run = (command, args, cwd) =>
  execFileSync(command, args, { stdio: "inherit", cwd });

for (const platform of Object.keys(PLATFORMS))
  assert(
    existsSync(resolve("artifacts/native", platform)),
    `Missing artifacts/native/${platform}; run release-native.mjs there`,
  );

const manifests = {
  "bindings/python/pyproject.toml": /^version = "([^"]+)"/m,
  "bindings/ruby/octri-spec-converter.gemspec": /spec\.version = '([^']+)'/,
  "bindings/dart/pubspec.yaml": /^version: (\S+)/m,
  "bindings/java/pom.xml":
    /<artifactId>spec-converter<\/artifactId>\s*<version>([^<]+)/,
  "bindings/kotlin/pom.xml":
    /<artifactId>spec-converter-kotlin<\/artifactId>\s*<version>([^<]+)/,
};
for (const [file, pattern] of Object.entries(manifests))
  assert.equal(
    readFileSync(file, "utf8").match(pattern)?.[1],
    version,
    `${file} must carry the workspace version ${version}`,
  );

/** Copy a binding directory into a fresh package directory with notices and changelog. */
function stage(language, target) {
  rmSync(target, { recursive: true, force: true });
  cpSync(resolve("bindings", language), target, {
    recursive: true,
    filter: (source) => !DEVELOPMENT.has(basename(source)),
  });
  for (const file of [...NOTICES, "CHANGELOG.md"])
    copyFileSync(file, join(target, basename(file)));
  return target;
}

const builders = {
  python() {
    const wheels = join(output, "python");
    mkdirSync(wheels, { recursive: true });
    for (const [platform, { shared, wheel }] of Object.entries(PLATFORMS)) {
      const work = stage(
        "python",
        resolve("artifacts/staging/python", platform),
      );
      const nativeDir = join(work, "src/octri_converter/native");
      mkdirSync(nativeDir, { recursive: true });
      copyFileSync(native(platform, shared), join(nativeDir, shared));
      for (const file of NOTICES)
        copyFileSync(file, join(nativeDir, basename(file)));
      const built = join(work, "dist");
      run("python3", ["-m", "build", "--wheel", "--outdir", built, work]);
      const [pure] = readdirSync(built);
      run("python3", [
        "-m",
        "wheel",
        "tags",
        "--remove",
        "--platform-tag",
        wheel,
        join(built, pure),
      ]);
      for (const file of readdirSync(built))
        copyFileSync(join(built, file), join(wheels, file));
    }
  },

  ruby() {
    const gems = join(output, "ruby");
    mkdirSync(gems, { recursive: true });
    for (const [platform, { shared, gem }] of Object.entries(PLATFORMS)) {
      const work = stage("ruby", resolve("artifacts/staging/ruby", platform));
      const nativeDir = join(work, "lib/octri_converter/native");
      mkdirSync(nativeDir, { recursive: true });
      copyFileSync(native(platform, shared), join(nativeDir, shared));
      for (const file of NOTICES)
        copyFileSync(file, join(nativeDir, basename(file)));
      run(
        "ruby",
        [
          "-rrubygems/package",
          "-e",
          "spec = Gem::Specification.load('octri-spec-converter.gemspec'); spec.platform = ARGV[0]; Gem::Package.build(spec)",
          gem,
        ],
        work,
      );
      const file = `octri-spec-converter-${version}-${gem}.gem`;
      copyFileSync(join(work, file), join(gems, file));
    }
  },

  /** Maven projects ready for `mvn -P release deploy`; the Java jar carries every platform. */
  maven() {
    const java = stage("java", join(output, "maven/java"));
    const resources = join(java, "src/main/resources");
    for (const [platform, { shared, jni }] of Object.entries(PLATFORMS)) {
      const target = join(resources, "dev/octri/converter/native", platform);
      mkdirSync(target, { recursive: true });
      copyFileSync(native(platform, shared), join(target, shared));
      copyFileSync(native(platform, jni), join(target, jni));
    }
    // The pom copies the binding's notices into META-INF itself.
    stage("kotlin", join(output, "maven/kotlin"));
  },

  dart() {
    const dart = stage("dart", join(output, "dart"));
    for (const [platform, { shared }] of Object.entries(PLATFORMS)) {
      const target = join(dart, "lib/src/native", platform);
      mkdirSync(target, { recursive: true });
      copyFileSync(native(platform, shared), join(target, shared));
    }
  },

  go() {
    const go = stage("go", join(output, "go"));
    copyFileSync(
      "bindings/c/include/octri_converter.h",
      join(go, "octri_converter.h"),
    );
    for (const [platform, { go: target }] of Object.entries(PLATFORMS)) {
      mkdirSync(join(go, "lib", target), { recursive: true });
      copyFileSync(
        native(platform, "liboctri_converter_ffi.a"),
        join(go, "lib", target, "liboctri_converter_ffi.a"),
      );
    }
  },

  php() {
    const php = stage("php", join(output, "php"));
    for (const [platform, { shared }] of Object.entries(PLATFORMS)) {
      mkdirSync(join(php, "native", platform), { recursive: true });
      copyFileSync(
        native(platform, shared),
        join(php, "native", platform, shared),
      );
    }
  },

  swift() {
    const swift = stage("swift", join(output, "swift"));
    for (const [platform, { shared }] of Object.entries(PLATFORMS)) {
      const target = join(swift, "Sources/OctriSpecConverter/native", platform);
      mkdirSync(target, { recursive: true });
      copyFileSync(native(platform, shared), join(target, shared));
    }
  },

  /** One archive per platform: header, shared and static library, notices. */
  c() {
    const archives = join(output, "c");
    mkdirSync(archives, { recursive: true });
    for (const [platform, { shared }] of Object.entries(PLATFORMS)) {
      const name = `octri-spec-converter-c-${version}-${platform}`;
      const work = resolve("artifacts/staging/c", name);
      rmSync(work, { recursive: true, force: true });
      mkdirSync(join(work, "include"), { recursive: true });
      mkdirSync(join(work, "lib"));
      copyFileSync(
        "bindings/c/include/octri_converter.h",
        join(work, "include/octri_converter.h"),
      );
      for (const file of [shared, "liboctri_converter_ffi.a"])
        copyFileSync(native(platform, file), join(work, "lib", file));
      for (const file of NOTICES)
        copyFileSync(file, join(work, basename(file)));
      run("tar", [
        "-czf",
        join(archives, `${name}.tar.gz`),
        "-C",
        resolve("artifacts/staging/c"),
        name,
      ]);
    }
  },
};

const requested = process.argv.slice(2);
for (const language of requested.length ? requested : Object.keys(builders)) {
  assert(builders[language], `Unknown package: ${language}`);
  rmSync(join(output, language), { recursive: true, force: true });
  builders[language]();
  console.log(`Assembled ${language} ${version}`);
}
