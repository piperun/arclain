/// Read pubspec YAML and Pub constraints without running project code.
library;

import 'dart:io';
import 'package:pub_semver/pub_semver.dart';
import 'package:yaml/yaml.dart';

Object? request(Map<String, dynamic> data) {
  switch (data['command']) {
    case 'read':
      final result = <String, Object?>{};
      for (final path in (data['files'] as List).cast<String>()) {
        final file = File(path);
        if (file.lengthSync() > 2 * 1024 * 1024) {
          throw FormatException('Pubspec exceeds 2 MiB: $path');
        }
        final value = loadYaml(file.readAsStringSync(), sourceUrl: file.uri);
        if (value is! Map) throw FormatException('Expected a YAML mapping: $path');
        result[path] = value;
      }
      return result;
    case 'allows':
      final constraint = VersionConstraint.parse(data['constraint'] as String);
      return (data['versions'] as List).cast<String>()
          .where((v) => constraint.allows(Version.parse(v))).toList();
    default:
      throw const FormatException('Unknown reader command');
  }
}
