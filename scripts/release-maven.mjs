/**
 * Publish one signed Maven project to Maven Central through the Portal API.
 *
 *   node scripts/release-maven.mjs <project dir>
 *
 * Run after `mvn -P release install`, which builds and signs the jars. The
 * central-publishing plugin's own upload was rejected in CI ("Bundle has
 * content that does NOT have a .pom file") while this bundle validates, so the
 * release builds it here: <group>/<artifact>/<version>/ with each file, its
 * .asc signature and .md5/.sha1 checksums. Needs MAVEN_CENTRAL_USERNAME and
 * MAVEN_CENTRAL_PASSWORD (a Central Portal user token).
 */
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const project = process.argv[2];
const pom = readFileSync(join(project, "pom.xml"), "utf8");
const field = (name) => pom.match(new RegExp(`<${name}>([^<]+)</${name}>`))[1];
const [group, artifact, version] = ["groupId", "artifactId", "version"].map(
  field,
);
const base = `${artifact}-${version}`;

const work = mkdtempSync(join(tmpdir(), "central-bundle-"));
const directory = join(work, ...group.split("."), artifact, version);
mkdirSync(directory, { recursive: true });
const files = [
  `${base}.pom`,
  `${base}.jar`,
  `${base}-sources.jar`,
  `${base}-javadoc.jar`,
];
for (const file of files) {
  const source = file.endsWith(".pom")
    ? join(project, "pom.xml")
    : join(project, "target", file);
  copyFileSync(source, join(directory, file));
  copyFileSync(
    join(project, "target", `${file}.asc`),
    join(directory, `${file}.asc`),
  );
  const bytes = readFileSync(join(directory, file));
  for (const algorithm of ["md5", "sha1"])
    writeFileSync(
      join(directory, `${file}.${algorithm}`),
      createHash(algorithm).update(bytes).digest("hex"),
    );
}
const bundle = join(work, "bundle.zip");
execFileSync("zip", ["-qr", bundle, group.split(".")[0]], { cwd: work });

const auth = {
  Authorization: `Bearer ${Buffer.from(
    `${process.env.MAVEN_CENTRAL_USERNAME}:${process.env.MAVEN_CENTRAL_PASSWORD}`,
  ).toString("base64")}`,
};
const api = "https://central.sonatype.com/api/v1/publisher";
const form = new FormData();
form.append("bundle", new Blob([readFileSync(bundle)]), "bundle.zip");
const upload = await fetch(
  `${api}/upload?name=${base}&publishingType=AUTOMATIC`,
  {
    method: "POST",
    headers: auth,
    body: form,
  },
);
if (!upload.ok)
  throw new Error(`Upload failed: ${upload.status} ${await upload.text()}`);
const deployment = (await upload.text()).trim();
console.log(
  `${group}:${artifact}:${version} uploaded as deployment ${deployment}`,
);

// Central validates, then publishes; publishing can take up to half an hour.
for (const deadline = Date.now() + 45 * 60_000; Date.now() < deadline; ) {
  const response = await fetch(`${api}/status?id=${deployment}`, {
    method: "POST",
    headers: auth,
  });
  const status = await response.json();
  if (status.deploymentState === "PUBLISHED") {
    console.log(`${group}:${artifact}:${version} published`);
    process.exit(0);
  }
  if (status.deploymentState === "FAILED")
    throw new Error(
      `Deployment ${deployment} failed: ${JSON.stringify(status.errors)}`,
    );
  await new Promise((resolve) => setTimeout(resolve, 15_000));
}
throw new Error(`Deployment ${deployment} did not publish within 45 minutes`);
