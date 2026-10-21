package dev.octri.converter;

import java.nio.file.Path;

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

  static native int abiVersion();

  static native byte[] call(byte[] input);

  private NativeBridge() {}
}
