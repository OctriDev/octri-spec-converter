Release builds copy the native library for each platform into `<platform>/` here
(`darwin-arm64`, `linux-x64`, `linux-arm64`). Development builds pass the
library path to `Converter(library:)` instead.
