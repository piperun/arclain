#!/usr/bin/env python3
"""Check that Wirt remains independent of product-specific code."""
from __future__ import annotations

import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

FORBIDDEN_PREFIXES = ("arclain_", "gameta_", "filer")
FORBIDDEN_EXACT = {
    "egui", "eframe", "flutter_rust_bridge", "frb",
    "dart_api", "allo_isolate",
}


@dataclass(frozen=True)
class RustToken:
    kind: str
    value: str | None
    line: int
    offset: int


@dataclass(frozen=True)
class WitToken:
    kind: str
    value: str
    line: int


def normalized(name: str) -> str:
    return name.replace("-", "_").lower()


def forbidden(name: str) -> bool:
    value = normalized(name)
    return value in FORBIDDEN_EXACT or value.startswith(FORBIDDEN_PREFIXES)


def dependency_tables(document: dict) -> list[dict]:
    tables = [
        document.get(name, {})
        for name in ("dependencies", "build-dependencies", "dev-dependencies")
    ]
    for target in document.get("target", {}).values():
        tables.extend(
            target.get(name, {})
            for name in ("dependencies", "build-dependencies", "dev-dependencies")
        )
    return tables


def workspace_dependencies(workspace_root: Path) -> dict:
    manifest = workspace_root / "Cargo.toml"
    if not manifest.exists():
        return {}
    with manifest.open("rb") as handle:
        document = tomllib.load(handle)
    return document.get("workspace", {}).get("dependencies", {})


def dependency_package(name: str, dependency: object, inherited: dict) -> str:
    if isinstance(dependency, dict) and dependency.get("workspace") is True:
        dependency = inherited.get(name, name)
    return dependency.get("package", name) if isinstance(dependency, dict) else name


def dependency_violations(workspace_root: Path) -> list[str]:
    manifest = workspace_root / "crates" / "wirt" / "Cargo.toml"
    if not manifest.exists():
        return ["crates/wirt/Cargo.toml: missing neutral crate manifest"]
    with manifest.open("rb") as handle:
        document = tomllib.load(handle)
    inherited = workspace_dependencies(workspace_root)
    names = []
    for table in dependency_tables(document):
        for name, dependency in table.items():
            package = dependency_package(name, dependency, inherited)
            if forbidden(package):
                names.append(package)
    names.sort()
    violations = [
        f"crates/wirt/Cargo.toml: forbidden dependency {name}" for name in names
    ]
    wasmtime = document.get("dependencies", {}).get("wasmtime")
    if wasmtime is not None and dependency_package("wasmtime", wasmtime, inherited) != "wasmtime":
        violations.append(
            "crates/wirt/Cargo.toml: wasmtime dependency must resolve to package wasmtime"
        )
    return violations


def scan_escaped_literal(
    source: str,
    start: int,
    line: int,
    name: str,
    *,
    delimiter: str = '"',
    single_character: bool = False,
    ascii_only: bool = False,
) -> tuple[str | None, int, int, str | None]:
    cursor = start + 1
    decoded: list[str] = []
    valid = True
    escapes = {
        "0": "\0",
        "t": "\t",
        "n": "\n",
        "r": "\r",
        '"': '"',
        "'": "'",
        "\\": "\\",
    }
    while cursor < len(source):
        value = source[cursor]
        if value == delimiter:
            decoded_value = "".join(decoded)
            if single_character and len(decoded_value) != 1:
                valid = False
            error = None if valid else f"malformed {name}"
            return (decoded_value if valid else None, cursor + 1, line, error)
        if value == "\n":
            if single_character:
                return None, cursor, line, f"unterminated {name}"
            decoded.append(value)
            line += 1
            cursor += 1
            continue
        if value != "\\":
            if ascii_only and ord(value) > 0x7F:
                valid = False
            decoded.append(value)
            cursor += 1
            continue

        cursor += 1
        if cursor >= len(source):
            return None, cursor, line, f"unterminated {name}"
        escaped = source[cursor]
        if escaped in escapes:
            decoded.append(escapes[escaped])
            cursor += 1
            continue
        if escaped == "\n":
            line += 1
            cursor += 1
            while cursor < len(source) and source[cursor].isspace():
                if source[cursor] == "\n":
                    line += 1
                cursor += 1
            continue
        if escaped == "x" and cursor + 2 < len(source):
            digits = source[cursor + 1 : cursor + 3]
            if all(character in "0123456789abcdefABCDEF" for character in digits):
                decoded.append(chr(int(digits, 16)))
                cursor += 3
                continue
        if escaped == "u" and cursor + 1 < len(source) and source[cursor + 1] == "{":
            end = source.find("}", cursor + 2)
            digits = source[cursor + 2 : end] if end != -1 else ""
            try:
                decoded.append(chr(int(digits.replace("_", ""), 16)))
            except (ValueError, OverflowError):
                valid = False
            if ascii_only:
                valid = False
            if end != -1:
                cursor = end + 1
                continue
        valid = False
        cursor += 1
    return None, cursor, line, f"unterminated {name}"


def raw_literal_at(source: str, start: int) -> tuple[str, str, str] | None:
    forms = (
        ("br", "literal", "raw byte string literal"),
        ("cr", "literal", "raw C string literal"),
        ("r", "string", "raw string literal"),
    )
    for prefix, kind, name in forms:
        cursor = start + len(prefix)
        if not source.startswith(prefix, start):
            continue
        while cursor < len(source) and source[cursor] == "#":
            cursor += 1
        if cursor < len(source) and source[cursor] == '"':
            return prefix, kind, name
    return None


def scan_raw_literal(
    source: str, start: int, line: int, prefix: str, name: str
) -> tuple[str | None, int, int, str | None]:
    cursor = start + len(prefix)
    while cursor < len(source) and source[cursor] == "#":
        cursor += 1

    hashes = source[start + len(prefix) : cursor]
    content_start = cursor + 1
    terminator = '"' + hashes
    end = source.find(terminator, content_start)
    if end == -1:
        value = source[content_start:]
        return (
            None,
            len(source),
            line + value.count("\n"),
            f"unterminated {name}",
        )
    value = source[content_start:end]
    if prefix == "br" and any(ord(character) > 0x7F for character in value):
        return (
            None,
            end + len(terminator),
            line + value.count("\n"),
            f"malformed {name}",
        )
    return value, end + len(terminator), line + value.count("\n"), None


def character_literal_at(source: str, start: int) -> bool:
    if start + 1 >= len(source):
        return True
    first = source[start + 1]
    if first == "\\" or first in "\r\n":
        return True
    if first.isalpha() or first == "_":
        cursor = start + 2
        while cursor < len(source) and (
            source[cursor].isalnum() or source[cursor] == "_"
        ):
            cursor += 1
        return cursor < len(source) and source[cursor] == "'"
    return True


def append_literal_token(
    tokens: list[RustToken],
    kind: str,
    value: str | None,
    error: str | None,
    line: int,
    offset: int,
) -> None:
    if error is not None:
        tokens.append(RustToken("error", error, line, offset))
    else:
        tokens.append(RustToken(kind, value, line, offset))


def rust_tokens(source: str) -> list[RustToken]:
    tokens: list[RustToken] = []
    cursor = 0
    line = 1
    while cursor < len(source):
        value = source[cursor]
        if value.isspace():
            if value == "\n":
                line += 1
            cursor += 1
            continue
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = len(source) if newline == -1 else newline
            continue
        if source.startswith("/*", cursor):
            comment_line = line
            comment_offset = cursor
            depth = 1
            cursor += 2
            while cursor < len(source) and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    if source[cursor] == "\n":
                        line += 1
                    cursor += 1
            if depth:
                tokens.append(
                    RustToken(
                        "error",
                        "unterminated block comment",
                        comment_line,
                        comment_offset,
                    )
                )
            continue

        token_line = line
        token_offset = cursor
        raw_literal = raw_literal_at(source, cursor)
        if raw_literal is not None:
            prefix, kind, name = raw_literal
            decoded, cursor, line, error = scan_raw_literal(
                source, cursor, line, prefix, name
            )
            append_literal_token(
                tokens, kind, decoded, error, token_line, token_offset
            )
            continue
        if source.startswith("b'", cursor):
            decoded, cursor, line, error = scan_escaped_literal(
                source,
                cursor + 1,
                line,
                "byte character literal",
                delimiter="'",
                single_character=True,
                ascii_only=True,
            )
            append_literal_token(
                tokens, "literal", decoded, error, token_line, token_offset
            )
            continue
        if value == "'" and character_literal_at(source, cursor):
            decoded, cursor, line, error = scan_escaped_literal(
                source,
                cursor,
                line,
                "character literal",
                delimiter="'",
                single_character=True,
            )
            append_literal_token(
                tokens, "literal", decoded, error, token_line, token_offset
            )
            continue
        if source.startswith('b"', cursor):
            decoded, cursor, line, error = scan_escaped_literal(
                source, cursor + 1, line, "byte string literal", ascii_only=True
            )
            append_literal_token(
                tokens, "literal", decoded, error, token_line, token_offset
            )
            continue
        if source.startswith('c"', cursor):
            decoded, cursor, line, error = scan_escaped_literal(
                source, cursor + 1, line, "C string literal"
            )
            append_literal_token(
                tokens, "literal", decoded, error, token_line, token_offset
            )
            continue
        if value == '"':
            decoded, cursor, line, error = scan_escaped_literal(
                source, cursor, line, "string literal"
            )
            append_literal_token(
                tokens, "string", decoded, error, token_line, token_offset
            )
            continue
        if value.isalpha() or value == "_":
            cursor += 1
            while cursor < len(source) and (
                source[cursor].isalnum() or source[cursor] == "_"
            ):
                cursor += 1
            tokens.append(
                RustToken("identifier", source[token_offset:cursor], token_line, token_offset)
            )
            continue
        tokens.append(RustToken("punctuation", value, token_line, token_offset))
        cursor += 1
    return tokens


def closing_token(
    tokens: list[RustToken], start: int, opening: str, closing: str
) -> int | None:
    depth = 0
    for index in range(start, len(tokens)):
        if tokens[index].value == opening:
            depth += 1
        elif tokens[index].value == closing:
            depth -= 1
            if depth == 0:
                return index
    return None


def import_issues(tokens: list[RustToken]) -> list[tuple[int, int, str]]:
    issues: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        is_use = token.kind == "identifier" and token.value == "use"
        is_extern = (
            token.kind == "identifier"
            and token.value == "extern"
            and index + 1 < len(tokens)
            and tokens[index + 1].kind == "identifier"
            and tokens[index + 1].value == "crate"
        )
        if not is_use and not is_extern:
            continue
        for candidate in tokens[index + 1 :]:
            if candidate.value == ";":
                break
            if candidate.kind == "identifier" and forbidden(candidate.value or ""):
                issues.append(
                    (
                        token.offset,
                        token.line,
                        f"forbidden import {candidate.value}",
                    )
                )
                break
    return issues


def compiled_path_issues(tokens: list[RustToken]) -> list[tuple[int, int, str | None]]:
    issues: list[tuple[int, int, str | None]] = []
    for index, token in enumerate(tokens):
        if token.value == "#":
            bracket = index + 1
            if bracket < len(tokens) and tokens[bracket].value == "!":
                bracket += 1
            if bracket >= len(tokens) or tokens[bracket].value != "[":
                continue
            end = closing_token(tokens, bracket, "[", "]")
            if end is None or bracket + 1 >= end:
                continue
            attribute = tokens[bracket + 1 : end]
            attribute_name = attribute[0].value
            if attribute_name not in {"path", "cfg_attr"}:
                continue
            for path_index, path_token in enumerate(attribute[:-2]):
                if path_token.value != "path" or attribute[path_index + 1].value != "=":
                    continue
                literal = attribute[path_index + 2]
                if literal.kind == "string":
                    issues.append((token.offset, path_token.line, literal.value))
                break

        if (
            token.kind == "identifier"
            and token.value == "include"
            and index + 2 < len(tokens)
            and tokens[index + 1].value == "!"
            and tokens[index + 2].value in {"(", "[", "{"}
        ):
            opening = tokens[index + 2].value or ""
            closing = {"(": ")", "[": "]", "{": "}"}[opening]
            end = closing_token(tokens, index + 2, opening, closing)
            if end is None:
                issues.append((token.offset, token.line, None))
                continue
            arguments = tokens[index + 3 : end]
            if arguments and arguments[-1].value == ",":
                arguments = arguments[:-1]
            literal = arguments[0] if len(arguments) == 1 else None
            issues.append(
                (
                    token.offset,
                    token.line,
                    literal.value if literal is not None and literal.kind == "string" else None,
                )
            )
    return issues


def has_double_colon(tokens: list[RustToken], index: int) -> bool:
    return (
        index + 1 < len(tokens)
        and tokens[index].value == ":"
        and tokens[index + 1].value == ":"
    )


def macro_path_before(
    tokens: list[RustToken], bang: int
) -> tuple[tuple[str, ...], int] | None:
    if bang == 0 or tokens[bang - 1].kind != "identifier":
        return None
    start = bang - 1
    while start >= 3 and has_double_colon(tokens, start - 2):
        if tokens[start - 3].kind != "identifier":
            return None
        start -= 3
    return tuple(token.value or "" for token in tokens[start:bang:3]), start


def bindgen_fields(
    tokens: list[RustToken],
) -> list[tuple[str, list[RustToken]]] | None:
    fields: list[tuple[str, list[RustToken]]] = []
    index = 0
    delimiters = {"(": ")", "[": "]", "{": "}"}
    while index < len(tokens):
        if tokens[index].kind != "identifier" or index + 1 >= len(tokens):
            return None
        name = tokens[index].value or ""
        if tokens[index + 1].value != ":":
            return None
        value_start = index + 2
        index = value_start
        stack: list[str] = []
        while index < len(tokens):
            value = tokens[index].value or ""
            if value in delimiters:
                stack.append(delimiters[value])
            elif stack and value == stack[-1]:
                stack.pop()
            elif value in {")", "]", "}"
            } or (value == "," and not stack):
                break
            index += 1
        if stack or index == value_start:
            return None
        fields.append((name, tokens[value_start:index]))
        if index == len(tokens):
            break
        if tokens[index].value != ",":
            return None
        index += 1
        if index == len(tokens):
            break
    return fields


def bindgen_path_input(
    arguments: list[RustToken], opening: str
) -> tuple[str | None, str | None]:
    if arguments and arguments[0].value == "{":
        end = closing_token(arguments, 0, "{", "}")
        if end is None or end != len(arguments) - 1:
            return None, "malformed component bindgen argument map"
        fields_tokens = arguments[1:end]
    else:
        return None, "component bindgen must use an inner braced argument map"

    fields = bindgen_fields(fields_tokens)
    if fields is None:
        return None, "malformed component bindgen argument map"
    names: set[str] = set()
    for name, _ in fields:
        if name in names:
            return None, f"component bindgen has duplicate input field: {name}"
        names.add(name)
    source_fields = [
        field for field in fields if field[0] in {"path", "inline", "interfaces"}
    ]
    if len(source_fields) != 1 or source_fields[0][0] != "path":
        return None, "component bindgen must use exactly one literal path input"
    value = source_fields[0][1]
    if len(value) != 1 or value[0].kind != "string":
        return None, "component bindgen must use exactly one literal path input"
    return value[0].value, None


def bindgen_declaration_issues(tokens: list[RustToken]) -> list[tuple[int, int, str]]:
    issues: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens):
        is_use = token.kind == "identifier" and token.value == "use"
        is_extern = (
            token.kind == "identifier"
            and token.value == "extern"
            and index + 1 < len(tokens)
            and tokens[index + 1].value == "crate"
        )
        if not is_use and not is_extern:
            continue
        start = index + 2 if is_extern else index + 1
        end = next(
            (candidate for candidate in range(start, len(tokens)) if tokens[candidate].value == ";"),
            len(tokens),
        )
        statement = tokens[start:end]
        identifiers = [candidate.value or "" for candidate in statement if candidate.kind == "identifier"]
        values = [candidate.value or "" for candidate in statement]
        aliases_reserved_name = any(
            values[position] == "as"
            and position + 1 < len(values)
            and values[position + 1] in {"wasmtime", "component", "bindgen"}
            for position in range(len(values))
        )
        references_bindgen = "bindgen" in identifiers
        component_alias = any(
            value == "component"
            and (
                position + 1 == len(values)
                or values[position + 1] in {"as", "}"}
            )
            for position, value in enumerate(values)
        )
        aliases_wasmtime_root = any(
            value == "wasmtime"
            and position + 1 < len(values)
            and values[position + 1] == "as"
            for position, value in enumerate(values)
        )
        has_wasmtime_glob = "wasmtime" in identifiers and "*" in values
        relevant = (
            (is_extern and "as" in identifiers)
            or references_bindgen
            or component_alias
            or aliases_wasmtime_root
            or aliases_reserved_name
            or has_wasmtime_glob
        )
        if relevant:
            issues.append(
                (
                    token.offset,
                    token.line,
                    "unsupported or ambiguous Wasmtime component use tree",
                )
            )
    return issues


def local_wasmtime_declaration_issues(
    tokens: list[RustToken],
) -> list[tuple[int, int, str]]:
    issues: list[tuple[int, int, str]] = []
    for index, token in enumerate(tokens[:-1]):
        if (
            token.kind == "identifier"
            and token.value == "mod"
            and tokens[index + 1].kind == "identifier"
            and tokens[index + 1].value == "wasmtime"
        ):
            issues.append(
                (
                    token.offset,
                    token.line,
                    "local wasmtime module declaration is not allowed",
                )
            )
    return issues


def component_bindgen_issues(
    tokens: list[RustToken], crate_root: Path, canonical_wit: Path
) -> list[tuple[int, int, str]]:
    issues: list[tuple[int, int, str]] = []
    declaration_issues = bindgen_declaration_issues(tokens)
    issues.extend(declaration_issues)
    issues.extend(local_wasmtime_declaration_issues(tokens))
    delimiters = {"(": ")", "[": "]", "{": "}"}
    direct_path = ("wasmtime", ":", ":", "component", ":", ":", "bindgen")

    for bang, token in enumerate(tokens):
        if token.value != "!":
            continue

        macro = macro_path_before(tokens, bang)
        if macro is None:
            continue
        path, invocation_start = macro
        if tuple(candidate.value for candidate in tokens[invocation_start:bang]) != direct_path:
            is_component_bindgen = path[-2:] == ("component", "bindgen")
            is_bare_bindgen = path == ("bindgen",)
            if (is_component_bindgen or is_bare_bindgen) and not declaration_issues:
                display = "::".join(path)
                issues.append(
                    (
                        tokens[invocation_start].offset,
                        tokens[invocation_start].line,
                        "unsupported or ambiguous component bindgen macro path: "
                        f"{display}",
                    )
                )
            continue

        if declaration_issues:
            continue

        opening_index = bang + 1
        if opening_index >= len(tokens) or tokens[opening_index].value not in delimiters:
            issues.append(
                (
                    tokens[invocation_start].offset,
                    tokens[invocation_start].line,
                    "component bindgen macro requires a delimited body",
                )
            )
            continue
        opening = tokens[opening_index].value or ""
        end = closing_token(tokens, opening_index, opening, delimiters[opening])
        if end is None:
            issues.append(
                (
                    tokens[invocation_start].offset,
                    tokens[invocation_start].line,
                    "unterminated component bindgen macro",
                )
            )
            continue

        arguments = tokens[opening_index + 1 : end]
        path_literal, argument_error = bindgen_path_input(arguments, opening)
        if argument_error is not None:
            issues.append(
                (
                    tokens[invocation_start].offset,
                    tokens[invocation_start].line,
                    argument_error,
                )
            )
            continue

        resolved = (crate_root / path_literal).resolve()
        if resolved != canonical_wit:
            issues.append(
                (
                    tokens[invocation_start].offset,
                    tokens[invocation_start].line,
                    "component bindgen path must resolve to wirt-sdk/wit/plugin.wit: "
                    f"{path_literal}",
                )
            )
    return issues


def path_is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def wit_tokens(source: str) -> list[WitToken]:
    """Tokenize enough of WIT to locate package declarations safely."""
    tokens: list[WitToken] = []
    cursor = 0
    line = 1
    while cursor < len(source):
        value = source[cursor]
        if value.isspace():
            if value == "\n":
                line += 1
            cursor += 1
            continue
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = len(source) if newline == -1 else newline
            continue
        if source.startswith("/*", cursor):
            comment_line = line
            depth = 1
            cursor += 2
            while cursor < len(source) and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    if source[cursor] == "\n":
                        line += 1
                    cursor += 1
            if depth:
                tokens.append(WitToken("error", "unterminated block comment", comment_line))
            continue
        token_line = line
        if value.isalnum() or value in {"_", "-"}:
            start = cursor
            cursor += 1
            while cursor < len(source) and (
                source[cursor].isalnum() or source[cursor] in {"_", "-"}
            ):
                cursor += 1
            tokens.append(WitToken("identifier", source[start:cursor], token_line))
            continue
        tokens.append(WitToken("punctuation", value, token_line))
        cursor += 1
    return tokens


def wit_package_declarations(source: str) -> tuple[list[tuple[str, str, str | None]], bool]:
    """Return WIT package declarations and whether any declaration is malformed."""
    tokens = wit_tokens(source)
    malformed = any(token.kind == "error" for token in tokens)
    declarations: list[tuple[str, str, str | None]] = []
    for index, token in enumerate(tokens):
        if token.kind != "identifier" or token.value != "package":
            continue
        cursor = index + 1
        if cursor >= len(tokens) or tokens[cursor].kind != "identifier":
            malformed = True
            continue
        namespace = tokens[cursor].value
        cursor += 1
        if cursor >= len(tokens) or tokens[cursor].value != ":":
            malformed = True
            continue
        cursor += 1
        if cursor >= len(tokens) or tokens[cursor].kind != "identifier":
            malformed = True
            continue
        name = tokens[cursor].value
        cursor += 1
        version = None
        if cursor < len(tokens) and tokens[cursor].value == "@":
            cursor += 1
            version_start = cursor
            while cursor < len(tokens) and tokens[cursor].value != ";":
                cursor += 1
            if cursor == version_start:
                malformed = True
                continue
            version = "".join(token.value for token in tokens[version_start:cursor])
        if cursor >= len(tokens) or tokens[cursor].value != ";":
            malformed = True
            continue
        declarations.append((namespace, name, version))
    if len(declarations) > 1:
        malformed = True
    return declarations, malformed


def wirt_wit_violations(workspace_root: Path) -> list[str]:
    wit_files = sorted(workspace_root.rglob("*.wit"))

    canonical = workspace_root / "wirt-sdk" / "wit" / "plugin.wit"
    plugin_wit_files = [path for path in wit_files if path.name == "plugin.wit"]
    violations: list[str] = []
    package_declarations = {
        path: wit_package_declarations(path.read_text(encoding="utf-8"))
        for path in wit_files
    }
    if canonical not in plugin_wit_files:
        violations.append("wirt-sdk/wit/plugin.wit: missing canonical Wirt plugin WIT")
    elif package_declarations[canonical] != ([("wirt", "plugin", "0.1.0")], False):
        violations.append(
            "wirt-sdk/wit/plugin.wit: must declare package wirt:plugin@0.1.0"
        )

    for path, (_, malformed) in package_declarations.items():
        if malformed and path != canonical:
            relative = path.relative_to(workspace_root).as_posix()
            violations.append(f"{relative}: malformed or ambiguous WIT package declaration")

    for path in plugin_wit_files:
        if path == canonical:
            continue
        relative = path.relative_to(workspace_root).as_posix()
        violations.append(
            f"{relative}: unexpected plugin WIT; only "
            "wirt-sdk/wit/plugin.wit is allowed"
        )

    for path in wit_files:
        if path == canonical:
            continue
        relative = path.relative_to(workspace_root).as_posix()
        declarations, _ = package_declarations[path]
        if path.name != "plugin.wit" and any(
            namespace == "wirt" and name == "plugin"
            for namespace, name, _ in declarations
        ):
            violations.append(
                f"{relative}: duplicate Wirt plugin WIT; only "
                "wirt-sdk/wit/plugin.wit is allowed"
            )
    return violations


def source_violations(workspace_root: Path) -> list[str]:
    source_root = workspace_root / "crates" / "wirt" / "src"
    crate_root = workspace_root / "crates" / "wirt"
    resolved_source_root = source_root.resolve()
    canonical_wit = (workspace_root / "wirt-sdk" / "wit" / "plugin.wit").resolve()
    violations: list[str] = []
    roots: list[Path] = []
    if source_root.exists():
        for candidate in sorted(source_root.rglob("*")):
            if not candidate.is_file():
                continue
            resolved = candidate.resolve()
            relative = candidate.relative_to(workspace_root).as_posix()
            if not path_is_within(resolved, resolved_source_root):
                violations.append(f"{relative}:1: source root file escapes crates/wirt/src")
                continue
            roots.append(resolved)
    pending = list(roots)
    seen: set[Path] = set()
    while pending:
        path = pending.pop(0).resolve()
        if path in seen:
            continue
        seen.add(path)
        relative = path.relative_to(workspace_root).as_posix()
        try:
            source = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            violations.append(
                f"{relative}:1: compiled source target is unreadable or not UTF-8"
            )
            continue
        tokens = rust_tokens(source)
        issues = [
            (token.offset, token.line, f"lexical error: {token.value}")
            for token in tokens
            if token.kind == "error"
        ]
        issues.extend(import_issues(tokens))
        issues.extend(component_bindgen_issues(tokens, crate_root, canonical_wit))
        for offset, line, literal in compiled_path_issues(tokens):
            if literal is None:
                issues.append((offset, line, "include! path is not a string literal"))
                continue
            resolved = (path.parent / literal).resolve()
            if not path_is_within(resolved, resolved_source_root):
                issues.append(
                    (
                        offset,
                        line,
                        f"compiled source path escapes crates/wirt/src: {literal}",
                    )
                )
                continue
            if not resolved.is_file():
                issues.append((offset, line, "compiled source target is missing or unreadable"))
                continue
            pending.append(resolved)
        for _, line, message in sorted(issues):
            violations.append(f"{relative}:{line}: {message}")
    return violations


def violations(workspace_root: Path) -> list[str]:
    return (
        dependency_violations(workspace_root)
        + wirt_wit_violations(workspace_root)
        + source_violations(workspace_root)
    )


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    found = violations(root)
    for violation in found:
        print(violation)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
