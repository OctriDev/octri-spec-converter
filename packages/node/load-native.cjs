// Development builds use the local addon. Published packages resolve a pinned,
// platform-specific dependency; installation never downloads executable code.
const { existsSync } = require("node:fs");
const { join } = require("node:path");

module.exports = function loadNative() {
  const local = join(__dirname, "octri-converter.node");
  if (existsSync(local)) return require(local);
  const platform = `${process.platform}-${process.arch}`;
  if (!["darwin-arm64", "linux-x64", "linux-arm64"].includes(platform))
    throw new Error(
      `Unsupported native platform: ${platform}. Use the WASM package or build from source.`,
    );
  if (
    process.platform === "linux" &&
    !process.report.getReport().header.glibcVersionRuntime
  )
    throw new Error(
      "Published Linux addons require glibc. Use WASM or build from source on musl.",
    );
  try {
    return require(`@octri/spec-converter-native-${platform}`);
  } catch (cause) {
    throw new Error(
      `Native dependency for ${platform} is missing or cannot load. Reinstall with optional dependencies enabled.`,
      { cause },
    );
  }
};
