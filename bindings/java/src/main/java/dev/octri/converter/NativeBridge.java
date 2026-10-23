package dev.octri.converter;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Locale;

final class NativeBridge {
  private static String loaded;

  static synchronized void load(String library, String bridge) {
    String paths =
        Path.of(library).toAbsolutePath().normalize()
            + "\n"
            + Path.of(bridge).toAbsolutePath().normalize();
    if (loaded != null && !loaded.equals(paths)) {
      throw new IllegalStateException(
          "One converter native library pair is supported per class loader");
    }
    if (loaded == null) {
      System.load(Path.of(library).toAbsolutePath().normalize().toString());
      System.load(Path.of(bridge).toAbsolutePath().normalize().toString());
      if (abiVersion() != 1) {
        throw new IllegalStateException("Expected converter ABI version 1");
      }
      loaded = paths;
    }
  }

  /** Extracts the library pair packaged for this platform and loads it once per class loader. */
  static synchronized void loadBundled() throws IOException {
    if (loaded != null) return;
    String os = System.getProperty("os.name").toLowerCase(Locale.ROOT);
    String arch = System.getProperty("os.arch");
    String platform =
        (os.contains("mac") ? "darwin" : os.contains("linux") ? "linux" : os)
            + "-"
            + (arch.equals("amd64") || arch.equals("x86_64")
                ? "x64"
                : arch.equals("aarch64") ? "arm64" : arch);
    String suffix = os.contains("mac") ? ".dylib" : ".so";
    Path directory = Files.createTempDirectory("octri-converter-");
    directory.toFile().deleteOnExit();
    Path library = extract(platform, "liboctri_converter_ffi" + suffix, directory);
    Path bridge = extract(platform, "liboctri_converter_jni" + suffix, directory);
    load(library.toString(), bridge.toString());
  }

  private static Path extract(String platform, String name, Path directory) throws IOException {
    try (InputStream input =
        NativeBridge.class.getResourceAsStream("native/" + platform + "/" + name)) {
      if (input == null) {
        throw new UnsatisfiedLinkError("No bundled converter native library for " + platform);
      }
      Path target = directory.resolve(name);
      Files.copy(input, target);
      target.toFile().deleteOnExit();
      return target;
    }
  }

  static native int abiVersion();

  static native byte[] call(byte[] input);

  private NativeBridge() {}
}
