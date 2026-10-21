import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

/** Check executable permissions in our Linux libraries and native consumers. */
export function checkElfPermissions(path) {
  if (process.platform !== "linux") return;
  const headers = execFileSync("readelf", ["-lW", path], {
    encoding: "utf8",
  });
  let stackFound = false;
  for (const line of headers.split("\n")) {
    const fields = line.trim().split(/\s+/);
    if (!["LOAD", "GNU_STACK"].includes(fields[0])) continue;
    const flags = fields.slice(6, -1).join("");
    assert(
      !(flags.includes("W") && flags.includes("E")),
      `${path}: writable executable segment`,
    );
    if (fields[0] === "GNU_STACK") {
      stackFound = true;
      assert(!flags.includes("E"), `${path}: executable stack`);
    }
  }
  assert(stackFound, `${path}: missing explicit stack permissions`);
}
