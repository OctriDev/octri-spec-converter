import 'dart:convert';
import 'dart:io';
import 'package:octri_spec_converter/octri_spec_converter.dart';

Future<void> main(List<String> args) async {
  final converter = Converter(args[0]);
  final raw = await stdin.transform(utf8.decoder).join();
  if (args.contains('--raw')) {
    stdout.write(converter.executeJson(raw));
    return;
  }
  final input =
      (jsonDecode(raw) as Map<String, dynamic>).cast<String, Object?>();
  Map<String, Object?> field(String name) =>
      (input[name] as Map<String, dynamic>).cast<String, Object?>();
  final action = input['action'];
  if (input['abiVersion'] == 1 &&
      [
        'convert',
        'review',
        'bundle',
        'compare',
        'export',
        'formats',
        'detect',
        'batch'
      ].contains(action)) {
    final Object? value = switch (action) {
      'convert' => converter.convert(field('request')),
      'review' => converter.review(field('request')),
      'bundle' => converter.bundle(field('request')),
      'detect' => converter.detect(field('request')),
      'batch' => converter.batch((input['requests'] as List)
          .map((item) => (item as Map<String, dynamic>).cast<String, Object?>())
          .toList()),
      'compare' => converter.compare(field('before'), field('after')),
      'export' =>
        converter.export(field('report'), format: input['format'] as String),
      _ => converter.formats(),
    };
    stdout.write(jsonEncode({'Ok': value}));
  } else {
    stdout.write(converter.callJson(input));
  }
}
