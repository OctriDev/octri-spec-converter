/** Package the static product with content hashes. No deployment or publication. */
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";

const output = resolve("artifacts/web");
const paths = [];
function collect(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) collect(path);
    else if (
      entry.isFile() &&
      !/\.d\.(?:ts|cts)$/.test(path) &&
      !entry.name.endsWith("-test.mjs") &&
      !["browser-test.mjs", "serve.mjs", "package.json"].includes(entry.name)
    )
      paths.push(path);
  }
}
if (!existsSync("packages/browser/pkg/octri_converter_bg.wasm"))
  throw new Error(
    "Build the WASM package before packaging the browser product",
  );
collect("web");
collect("packages/browser");
paths.push(
  "LICENSE",
  "THIRD-PARTY-NOTICES.md",
  "vendor/PARSER-LICENSE",
  "vendor/AUDITOR-LICENSE",
  "vendor/octri-parser/schemas/OAI-LICENSE",
);
rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });
const manifest = {
  schemaVersion: "1.0.0",
  engineVersion: JSON.parse(readFileSync("package.json", "utf8")).version,
  entry: "web/index.html",
  files: {},
};
for (const path of paths.sort()) {
  mkdirSync(dirname(join(output, path)), { recursive: true });
  cpSync(path, join(output, path));
  manifest.files[path] = createHash("sha256")
    .update(readFileSync(path))
    .digest("hex");
}
writeFileSync(
  join(output, "manifest.json"),
  JSON.stringify(manifest, null, 2) + "\n",
);
cpSync("packages/web/package.json", join(output, "package.json"));
cpSync("packages/web/README.md", join(output, "README.md"));
console.log(`Packaged ${paths.length} verified static files: ${output}`);

const installIndex = process.argv.indexOf("--octri-root");
if (installIndex !== -1) {
  const octriRoot = resolve(process.argv[installIndex + 1] || "");
  if (
    JSON.parse(readFileSync(join(octriRoot, "package.json"), "utf8")).name !==
    "octri"
  )
    throw new Error("--octri-root must identify the Octri workspace");
  for (const app of ["web", "marketing"]) {
    const appRoot = join(octriRoot, "apps", app);
    if (
      JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8")).name !==
      `@octri/${app}`
    )
      throw new Error(`Missing Octri ${app} application`);
    const destination = join(appRoot, "public/spec-converter");
    rmSync(destination, { recursive: true, force: true });
    cpSync(output, destination, { recursive: true });
    console.log(`Installed generated converter assets in ${destination}`);
  }
}
