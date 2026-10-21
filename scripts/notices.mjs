import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, readdirSync, cpSync } from "node:fs";
import { dirname, join } from "node:path";
import { rustEnvironment } from "./toolchain.mjs";

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--locked", "--format-version", "1"], {
    env: rustEnvironment(),
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  }),
);
const dependencies = metadata.packages
  .filter((pkg) => pkg.source)
  .sort((a, b) =>
    `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`),
  );
const sections = [
  "# Third-party notices\n\nGenerated from the locked Cargo dependency graph, including build and platform-specific dependencies.\nVendored OpenAPI schemas retain their OpenAPI Initiative Apache-2.0 notice under vendor/octri-parser/schemas.\n\n| Crate | Version | License |\n| --- | --- | --- |",
];
for (const pkg of dependencies)
  sections.push(
    `| ${pkg.name} | ${pkg.version} | ${pkg.license ?? "See bundled license file"} |`,
  );
for (const pkg of dependencies) {
  const directory = dirname(pkg.manifest_path);
  const files = new Set(
    readdirSync(directory).filter((name) =>
      /^(licen[sc]e|copying|notice)(\.|-|$)/i.test(name),
    ),
  );
  if (pkg.license_file) files.add(pkg.license_file);
  sections.push(
    `\n## ${pkg.name} ${pkg.version}\n\nSource: ${pkg.repository ?? pkg.source}\n`,
  );
  for (const filename of [...files].sort()) {
    try {
      sections.push(
        `\n### ${filename}\n\n\`\`\`text\n${readFileSync(join(directory, filename), "utf8").trim()}\n\`\`\`\n`,
      );
    } catch (error) {
      if (error.code !== "EISDIR") throw error;
    }
  }
}
writeFileSync("THIRD-PARTY-NOTICES.md", sections.join("\n") + "\n");
for (const directory of [
  "packages/node",
  "packages/browser",
  ...["python", "ruby", "php", "dart", "go", "swift", "java", "kotlin"].map(
    (name) => `bindings/${name}`,
  ),
]) {
  cpSync("LICENSE", `${directory}/LICENSE`);
  cpSync("THIRD-PARTY-NOTICES.md", `${directory}/THIRD-PARTY-NOTICES.md`);
  cpSync("vendor/octri-parser/schemas/OAI-LICENSE", `${directory}/OAI-LICENSE`);
}
console.log(`Recorded ${dependencies.length} locked dependency notices.`);
