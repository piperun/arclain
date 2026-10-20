/// Read pubspec YAML and Pub version constraints without running project code.
library;

import 'dart:convert';
import 'dart:io';
import 'package:monodev_pub_reader/reader.dart';

Future<void> main() async {
  try {
    final data = jsonDecode(await stdin.transform(utf8.decoder).join());
    if (data is! Map<String, dynamic>) {
      throw const FormatException('Expected a JSON object');
    }
    stdout.writeln(jsonEncode(request(data)));
  } catch (error) {
    stderr.writeln('Pub metadata failed: $error');
    exitCode = 1;
  }
}
