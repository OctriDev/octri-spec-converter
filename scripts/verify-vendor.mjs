import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";

function files(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink())
      throw new Error(`Vendored sources cannot contain symlinks: ${path}`);
    return entry.isDirectory() ? files(path) : [path];
  });
}
const hashes = Object.fromEntries(
  files("vendor")
    .filter((path) => path !== join("vendor", "snapshot.json"))
    .sort()
    .map((path) => [
      relative("vendor", path).split(sep).join("/"),
      createHash("sha256").update(readFileSync(path)).digest("hex"),
    ]),
);
if (process.argv.includes("--update")) {
  writeFileSync(
    "vendor/snapshot.json",
    JSON.stringify(
      {
        sources: [
          "https://github.com/octridev/octri-openapi-parser/tree/main/crates/octri-parser",
          "https://github.com/octridev/octri-spec-auditor/tree/main/crates/octri-auditor",
        ],
        provenance:
          "Vendored source snapshots pinned by SHA-256. Snapshot hashes identify the exact imported files; release qualification and source review are separate gates.",
        files: hashes,
      },
      null,
      2,
    ) + "\n",
  );
} else {
  const snapshot = JSON.parse(readFileSync("vendor/snapshot.json", "utf8"));
  if (JSON.stringify(snapshot.files) !== JSON.stringify(hashes))
    throw new Error(
      "Vendored sources differ from the pinned snapshot. Review upstream changes and explicitly update the manifest.",
    );
  // octri-converter embeds this schema from inside its own crate.
  for (const file of ["openapi-3.0.json", "OAI-LICENSE"])
    if (
      !readFileSync(`crates/octri-converter/schemas/${file}`).equals(
        readFileSync(`vendor/octri-parser/schemas/${file}`),
      )
    )
      throw new Error(
        `crates/octri-converter/schemas/${file} differs from the vendored parser's copy.`,
      );
  console.log(`Verified ${Object.keys(hashes).length} vendored files.`);
}
