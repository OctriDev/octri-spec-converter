/** Package one native addon and the portable Node facade. No upload. */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";

const metadata = JSON.parse(readFileSync("packages/node/package.json", "utf8"));
const platform = `${process.platform}-${process.arch}`;
assert(
  ["darwin-arm64", "linux-x64", "linux-arm64"].includes(platform),
  `Unsupported native npm target: ${platform}`,
);
const output = resolve("artifacts/npm-native", platform);
const staging = join(output, "platform");
rmSync(output, { recursive: true, force: true });
mkdirSync(staging, { recursive: true });
const name = `${metadata.name}-${platform}`;
const platformMetadata = {
  name,
  version: metadata.version,
  description: `Platform addon for ${metadata.name} (${platform})`,
  license: "Apache-2.0",
  main: "binding.node",
  files: ["binding.node", "LICENSE", "OAI-LICENSE", "THIRD-PARTY-NOTICES.md"],
  os: [process.platform],
  cpu: [process.arch],
  ...(process.platform === "linux" ? { libc: ["glibc"] } : {}),
  keywords: [...metadata.keywords, process.platform, process.arch],
  repository: metadata.repository,
  homepage: metadata.homepage,
  bugs: metadata.bugs,
  publishConfig: { access: "public" },
};
copyFileSync(
  "packages/node/octri-converter.node",
  join(staging, "binding.node"),
);
for (const file of ["LICENSE", "OAI-LICENSE", "THIRD-PARTY-NOTICES.md"])
  copyFileSync(join("packages/node", file), join(staging, file));
writeFileSync(
  join(staging, "package.json"),
  JSON.stringify(platformMetadata, null, 2) + "\n",
);
writeFileSync(
  join(staging, "README.md"),
  `# ${name}\n\n[Live demo](${metadata.homepage}) · Free to try in your browser, no account required.\n\nPlatform-specific Rust Node-API binary for [${metadata.name}](https://www.npmjs.com/package/${metadata.name}). Install the parent package with optional dependencies enabled:\n\n\`\`\`sh\nnpm install ${metadata.name}\n\`\`\`\n\nRequires Node.js 22.19 or later, ${process.platform} ${process.arch}${process.platform === "linux" ? " with glibc 2.36 or later" : ""}. No install-time scripts or binary downloads. Apache-2.0; retain bundled third-party notices.\n`,
);
const pack = (directory) =>
  JSON.parse(
    execFileSync(
      "npm",
      [
        "pack",
        directory,
        "--ignore-scripts",
        "--json",
        "--pack-destination",
        output,
      ],
      { encoding: "utf8" },
    ),
  )[0];
const native = pack(staging);
const facade = pack(resolve("packages/node"));
assert(native.files.some((file) => file.path === "README.md"));
assert(facade.files.some((file) => file.path === "README.md"));
assert(!facade.files.some((file) => file.path.endsWith(".node")));
writeFileSync(
  join(output, "manifest.json"),
  JSON.stringify(
    {
      platform,
      version: metadata.version,
      packages: [native, facade].map(({ name, filename, integrity }) => ({
        name,
        filename,
        integrity,
      })),
    },
    null,
    2,
  ) + "\n",
);
console.log(`Packed native addon and facade: ${output}`);
