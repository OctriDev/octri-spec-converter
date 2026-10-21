import 'dart:convert';

class LosslessNumberException extends RangeError {
  final String code = 'LOSSLESS_NUMBER_REQUIRED';
  LosslessNumberException()
      : super(
            'Decoded Dart values would change an exact JSON number. Use executeJson or callJson.');
}

String? _decimal(String token) {
  final parts = token.toLowerCase().split('e');
  final negative = parts[0].startsWith('-');
  final unsigned = negative ? parts[0].substring(1) : parts[0];
  final dot = unsigned.indexOf('.');
  final digits = unsigned.replaceAll('.', '').replaceFirst(RegExp(r'^0+'), '');
  if (digits.isEmpty) return '0';
  final exponent = parts.length > 1 ? parts[1] : '0';
  if (exponent.replaceFirst(RegExp(r'^[+-]?0*'), '').length > 15) return null;
  var power = int.parse(exponent) - (dot < 0 ? 0 : unsigned.length - dot - 1);
  final coefficient = digits.replaceFirst(RegExp(r'0+$'), '');
  power += digits.length - coefficient.length;
  return '${negative ? '-' : ''}${coefficient}e$power';
}

/// Validate numeric round trips before Dart's JSON decoder can round them.
Map<String, Object?> decodeExactJson(String raw) {
  final tokens =
      RegExp(r'"(?:[^"\\]|\\.)*"|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?');
  final low = BigInt.parse('-9223372036854775808');
  final high = BigInt.parse('9223372036854775807');
  for (final match in tokens.allMatches(raw)) {
    final token = match.group(0)!;
    if (token.startsWith('"')) continue;
    if (token.contains(RegExp('[.eE]'))) {
      final value = double.parse(token);
      if (!value.isFinite ||
          _decimal(token) == null ||
          _decimal(token) != _decimal(jsonEncode(value))) {
        throw LosslessNumberException();
      }
    } else {
      final integer = BigInt.parse(token);
      if (integer < low || integer > high) throw LosslessNumberException();
    }
  }
  return (jsonDecode(raw) as Map<String, dynamic>).cast<String, Object?>();
}
