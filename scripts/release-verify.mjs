/**
 * Install the assembled packages the way a user would and list the supported formats with the
 * library each one bundles. No consumer is told where the native library is.
 *
 *   node scripts/release-verify.mjs [python|ruby|maven|dart|go|php|swift|c ...]
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { version } from "./native-build.mjs";

const packages = resolve("artifacts/packages");
const platform = `${process.platform}-${process.arch}`;
const run = (command, args, options = {}) =>
  execFileSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    ...options,
  });

const consumers = {
  python(directory) {
    run("python3", ["-m", "venv", join(directory, "env")]);
    const python = join(directory, "env/bin/python");
    // pip picks the wheel by platform tag, which is what a user's install does.
    run(python, [
      "-m",
      "pip",
      "install",
      "--no-index",
      "--find-links",
      join(packages, "python"),
      `octri-spec-converter==${version}`,
    ]);
    run(python, [
      "-c",
      "from octri_converter import Converter;assert Converter().formats()",
    ]);
  },

  ruby(directory) {
    const gems = join(directory, "gems");
    // A local install cannot choose among platform gems, so ask RubyGems which
    // one this host matches, as rubygems.org does, and require exactly one.
    const gem = run("ruby", [
      "-rrubygems/package",
      "-e",
      "m = Dir[File.join(ARGV[0], '*.gem')].select { |f| Gem::Platform.match_spec?(Gem::Package.new(f).spec) }; abort(m.inspect) unless m.size == 1; print m[0]",
      join(packages, "ruby"),
    ]);
    run("gem", ["install", "--no-document", "--install-dir", gems, gem]);
    run(
      "ruby",
      [
        "-e",
        "require 'octri_converter';abort if OctriConverter::Converter.new.formats.empty?",
      ],
      { env: { ...process.env, GEM_HOME: gems, GEM_PATH: gems } },
    );
  },

  maven(directory) {
    const repository = `-Dmaven.repo.local=${join(directory, "m2")}`;
    for (const project of ["java", "kotlin"])
      run("mvn", [
        "-q",
        repository,
        "-f",
        join(packages, "maven", project, "pom.xml"),
        "install",
        "-DskipTests",
      ]);
    const classpath = join(directory, "classpath.txt");
    run("mvn", [
      "-q",
      repository,
      "-f",
      join(packages, "maven/kotlin/pom.xml"),
      "dependency:build-classpath",
      `-Dmdep.outputFile=${classpath}`,
    ]);
    const cp = [
      join(
        packages,
        "maven/kotlin/target",
        `spec-converter-kotlin-${version}.jar`,
      ),
      readFileSync(classpath, "utf8").trim(),
    ].join(delimiter);
    writeFileSync(
      join(directory, "Consumer.java"),
      "class Consumer{public static void main(String[]a)throws Exception{" +
        'if(new dev.octri.converter.Converter().formats().isEmpty())throw new AssertionError("java");' +
        'if(new dev.octri.converter.kotlin.Converter().formats().isEmpty())throw new AssertionError("kotlin");}}',
    );
    run("javac", ["--release", "11", "-cp", cp, "Consumer.java"], {
      cwd: directory,
    });
    run("java", [
      "-Xcheck:jni",
      "-cp",
      [directory, cp].join(delimiter),
      "Consumer",
    ]);
  },

  dart(directory) {
    writeFileSync(
      join(directory, "pubspec.yaml"),
      `name: consumer\nenvironment:\n  sdk: '>=3.4.0 <4.0.0'\ndependencies:\n  octri_spec_converter:\n    path: ${join(packages, "dart")}\n`,
    );
    writeFileSync(
      join(directory, "main.dart"),
      "import 'package:octri_spec_converter/octri_spec_converter.dart';void main(){if(Converter().formats().isEmpty)throw StateError('consumer');}\n",
    );
    run("dart", ["pub", "get"], { cwd: directory });
    run("dart", ["run", "main.dart"], { cwd: directory });
  },

  go(directory) {
    const module = "github.com/octridev/octri-spec-converter-go";
    writeFileSync(
      join(directory, "go.mod"),
      `module consumer\n\ngo 1.23\nrequire ${module} v${version}\nreplace ${module} => ${join(packages, "go")}\n`,
    );
    writeFileSync(
      join(directory, "main.go"),
      `package main\nimport a "${module}"\nfunc main(){r,e:=(a.Converter{}).Formats();if e!=nil||len(r)<3{panic(e)}}\n`,
    );
    const env = { ...process.env, CGO_ENABLED: "1", GOWORK: "off" };
    delete env.CGO_LDFLAGS;
    run("go", ["build", "-o", "consumer", "."], { cwd: directory, env });
    run(join(directory, "consumer"), []);
  },

  php(directory) {
    writeFileSync(
      join(directory, "composer.json"),
      JSON.stringify({
        repositories: [
          {
            type: "path",
            url: join(packages, "php"),
            options: { symlink: false },
          },
        ],
        require: { "octri/spec-converter": "*" },
        "minimum-stability": "dev",
      }),
    );
    run(
      "composer",
      ["install", "--no-interaction", "--no-plugins", "--no-scripts"],
      {
        cwd: directory,
      },
    );
    writeFileSync(
      join(directory, "consumer.php"),
      "<?php require __DIR__.'/vendor/autoload.php'; if(!(array)(new \\Octri\\Convert\\Converter())->formats())throw new RuntimeException('consumer');",
    );
    run("php", ["-d", "ffi.enable=true", "consumer.php"], {
      cwd: directory,
    });
  },

  swift(directory) {
    mkdirSync(join(directory, "Sources/Consumer"), { recursive: true });
    writeFileSync(
      join(directory, "Package.swift"),
      `// swift-tools-version: 6.0\nimport PackageDescription\nlet package=Package(name:"Consumer",platforms:[.macOS(.v13)],dependencies:[.package(path:"${join(packages, "swift")}")],targets:[.executableTarget(name:"Consumer",dependencies:[.product(name:"OctriSpecConverter",package:"swift")])])\n`,
    );
    writeFileSync(
      join(directory, "Sources/Consumer/main.swift"),
      'import OctriSpecConverter\nguard case .object(let formats)=try Converter().formats(),!formats.isEmpty else{fatalError("consumer")}\n',
    );
    run(
      "swift",
      [
        "run",
        "-c",
        "release",
        // The Swift image's default gold linker rejects separate-code; lld takes it.
        ...(process.platform === "linux"
          ? [
              "-Xswiftc",
              "-use-ld=lld",
              "-Xlinker",
              "-z",
              "-Xlinker",
              "separate-code",
            ]
          : []),
        "Consumer",
      ],
      { cwd: directory },
    );
  },

  c(directory) {
    const name = `octri-spec-converter-c-${version}-${platform}`;
    run("tar", [
      "-xzf",
      join(packages, "c", `${name}.tar.gz`),
      "-C",
      directory,
    ]);
    const root = join(directory, name);
    run("clang", [
      "-std=c11",
      `-I${join(root, "include")}`,
      resolve("bindings/c/check.c"),
      `-L${join(root, "lib")}`,
      "-loctri_converter_ffi",
      `-Wl,-rpath,${join(root, "lib")}`,
      "-o",
      join(directory, "check"),
    ]);
    run(join(directory, "check"), []);
  },
};

const requested = process.argv.slice(2);
for (const language of requested.length ? requested : Object.keys(consumers)) {
  assert(consumers[language], `Unknown package: ${language}`);
  const directory = mkdtempSync(join(tmpdir(), `octri-${language}-consumer-`));
  try {
    consumers[language](directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
  console.log(`Installed ${language} ${version} package works on ${platform}`);
}
