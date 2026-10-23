import 'dart:convert';
import 'exact_json.dart';
export 'exact_json.dart' show LosslessNumberException;
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

final class _Buffer extends Opaque {}

typedef _VersionNative = Uint32 Function();
typedef _Version = int Function();
typedef _NewNative = Pointer<_Buffer> Function(UintPtr);
typedef _New = Pointer<_Buffer> Function(int);
typedef _DataNative = Pointer<Uint8> Function(Pointer<_Buffer>);
typedef _Data = Pointer<Uint8> Function(Pointer<_Buffer>);
typedef _LenNative = UintPtr Function(Pointer<_Buffer>);
typedef _Len = int Function(Pointer<_Buffer>);
typedef _FreeNative = Void Function(Pointer<_Buffer>);
typedef _Free = void Function(Pointer<_Buffer>);
typedef _CallNative = Pointer<_Buffer> Function(Pointer<Uint8>, UintPtr);
typedef _Call = Pointer<_Buffer> Function(Pointer<Uint8>, int);

class ConversionException implements Exception {
  final String code;
  final String message;
  final List<Object?> diagnostics;
  ConversionException(this.message, this.diagnostics, {required this.code});
  @override
  String toString() => 'ConversionException: $message';
}

/// Dart native VM binding. Calls block this isolate; run large work in a worker
/// isolate. Conversion semantics and artifact validation remain in Rust.
/// The library stays loaded for the isolate's lifetime; buffers are freed per call.
class Converter {
  final DynamicLibrary _library;
  late final _New _new = _library.lookupFunction<_NewNative, _New>(
    'octri_converter_buffer_new',
  );
  late final _Data _data = _library.lookupFunction<_DataNative, _Data>(
    'octri_converter_buffer_data',
  );
  late final _Len _length = _library.lookupFunction<_LenNative, _Len>(
    'octri_converter_buffer_len',
  );
  late final _Free _free = _library.lookupFunction<_FreeNative, _Free>(
    'octri_converter_buffer_free',
  );
  late final _Call _call = _library.lookupFunction<_CallNative, _Call>(
    'octri_converter_call',
  );

  /// Without a path, loads the library this package bundles for the host
  /// platform. Compiled executables cannot resolve package files; pass the
  /// library path there.
  Converter([String? library])
      : _library = DynamicLibrary.open(
            File(library ?? _bundledLibrary()).resolveSymbolicLinksSync()) {
    final version = _library.lookupFunction<_VersionNative, _Version>(
      'octri_converter_abi_version',
    );
    if (version() != 1) throw StateError('Expected Octri native ABI version 1');
  }

  static String _bundledLibrary() {
    final arch = switch (Abi.current()) {
      Abi.linuxX64 || Abi.macosX64 => 'x64',
      Abi.linuxArm64 || Abi.macosArm64 => 'arm64',
      final abi => '$abi',
    };
    final os = Platform.isMacOS ? 'darwin' : Platform.operatingSystem;
    final file = Platform.isMacOS
        ? 'liboctri_converter_ffi.dylib'
        : 'liboctri_converter_ffi.so';
    final uri = Isolate.resolvePackageUriSync(
        Uri.parse('package:octri_spec_converter/src/native/$os-$arch/$file'));
    if (uri == null)
      throw UnsupportedError(
          'Package files are unavailable; pass the native library path');
    return uri.toFilePath();
  }

  String callJson(Map<String, Object?> envelope) {
    return executeJson(jsonEncode(envelope));
  }

  /// Transport original JSON without decoding numbers through Dart's host model.
  String executeJson(String envelope) {
    final bytes = utf8.encode(envelope);
    if (bytes.length > 128 * 1024 * 1024)
      throw ArgumentError('Binding request exceeds 128 MiB');
    final input = _new(bytes.length);
    if (input == nullptr) throw StateError('Native input allocation failed');
    try {
      _data(input).asTypedList(bytes.length).setAll(0, bytes);
      final output = _call(_data(input), bytes.length);
      if (output == nullptr)
        throw StateError('Native converter returned a null buffer');
      try {
        // UTF-8 decoding copies all bytes before native output is freed.
        final text = utf8.decode(_data(output).asTypedList(_length(output)));
        return text;
      } finally {
        _free(output);
      }
    } finally {
      _free(input);
    }
  }

  /// Decoded values follow Dart's number model. callJson preserves native JSON
  /// exactly when unsigned integers outside the signed range are required.
  Map<String, Object?> call(Map<String, Object?> envelope) {
    return decodeExactJson(callJson(envelope));
  }

  Object? _result(Map<String, Object?> envelope) {
    final response = call(envelope);
    if (response.containsKey('Err')) {
      final error = response['Err'] as Map<String, dynamic>;
      throw ConversionException(
        error['message'] as String,
        (error['diagnostics'] as List).cast<Object?>(),
        code: error['code'] as String,
      );
    }
    if (!response.containsKey('Ok'))
      throw StateError('Invalid native response envelope');
    return response['Ok'];
  }

  Map<String, Object?> convert(Map<String, Object?> request) =>
      (_result({'abiVersion': 1, 'action': 'convert', 'request': request})
              as Map<String, dynamic>)
          .cast<String, Object?>();
  Map<String, Object?> review(Map<String, Object?> request) =>
      (_result({'abiVersion': 1, 'action': 'review', 'request': request})
              as Map<String, dynamic>)
          .cast<String, Object?>();
  Map<String, Object?> compare(
          Map<String, Object?> before, Map<String, Object?> after) =>
      (_result({
        'abiVersion': 1,
        'action': 'compare',
        'before': before,
        'after': after
      }) as Map<String, dynamic>)
          .cast<String, Object?>();
  Object? export(Map<String, Object?> report, {String format = 'json'}) =>
      _result({
        'abiVersion': 1,
        'action': 'export',
        'report': report,
        'format': format
      });
  Map<String, Object?> formats() =>
      (_result({'abiVersion': 1, 'action': 'formats'}) as Map<String, dynamic>)
          .cast<String, Object?>();
  Map<String, Object?> bundle(Map<String, Object?> request) =>
      (_result({'abiVersion': 1, 'action': 'bundle', 'request': request})
              as Map<String, dynamic>)
          .cast<String, Object?>();
  String detect(Map<String, Object?> request) =>
      _result({'abiVersion': 1, 'action': 'detect', 'request': request})
          as String;
  List<Object?> batch(List<Map<String, Object?>> requests) =>
      (_result({'abiVersion': 1, 'action': 'batch', 'requests': requests})
              as List)
          .cast<Object?>();
  static Map<String, Object?> text(String content,
          {String uri = 'memory://convert/openapi.yaml'}) =>
      {'uri': uri, 'content': content, 'generated': false};
}
