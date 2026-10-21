import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

// Use standard rustup or an optional isolated toolchain cache.
export function rustEnvironment() {
  const local = join(homedir(), ".cache/octri-rust");
  if (existsSync(join(local, "cargo/bin/rustup"))) {
    const env = {
      ...process.env,
      RUSTUP_HOME: join(local, "rustup"),
      PATH: `${join(local, "cargo/bin")}:${process.env.PATH}`,
    };
    env.RUSTC = execFileSync(
      join(local, "cargo/bin/rustup"),
      ["which", "rustc"],
      { env, encoding: "utf8" },
    ).trim();
    env.RUSTDOC = execFileSync(
      join(local, "cargo/bin/rustup"),
      ["which", "rustdoc"],
      { env, encoding: "utf8" },
    ).trim();
    return env;
  }
  return process.env;
}
