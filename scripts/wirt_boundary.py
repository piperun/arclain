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
    return [f"crates/wirt/Cargo.toml: forbidden dependency {name}" for name in names]


def scan_normal_string(source: str, start: int, line: int) -> tuple[str | None, int, int]:
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
        if value == '"':
            return ("".join(decoded) if valid else None, cursor + 1, line)
        if value == "\n":
            decoded.append(value)
            line += 1
            cursor += 1
            continue
        if value != "\\":
            decoded.append(value)
            cursor += 1
            continue

        cursor += 1
        if cursor >= len(source):
            return None, cursor, line
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
            if end != -1:
                cursor = end + 1
                continue
        valid = False
        cursor += 1
    return None, cursor, line


def scan_raw_string(
    source: str, start: int, line: int
) -> tuple[str, int, int] | None:
    if source[start] != "r":
        return None
    cursor = start + 1
    while cursor < len(source) and source[cursor] == "#":
        cursor += 1
    if cursor >= len(source) or source[cursor] != '"':
        return None

    hashes = source[start + 1 : cursor]
    content_start = cursor + 1
    terminator = '"' + hashes
    end = source.find(terminator, content_start)
    if end == -1:
        value = source[content_start:]
        return value, len(source), line + value.count("\n")
    value = source[content_start:end]
    return value, end + len(terminator), line + value.count("\n")


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
            continue

        token_line = line
        token_offset = cursor
        raw_string = scan_raw_string(source, cursor, line)
        if raw_string is not None:
            decoded, cursor, line = raw_string
            tokens.append(RustToken("string", decoded, token_line, token_offset))
            continue
        if value == '"':
            decoded, cursor, line = scan_normal_string(source, cursor, line)
            tokens.append(RustToken("string", decoded, token_line, token_offset))
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
            and tokens[index + 2].value == "("
        ):
            end = closing_token(tokens, index + 2, "(", ")")
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


def path_is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def source_violations(workspace_root: Path) -> list[str]:
    source_root = workspace_root / "crates" / "wirt" / "src"
    crate_root = workspace_root / "crates" / "wirt"
    resolved_crate_root = crate_root.resolve()
    violations: list[str] = []
    for path in sorted(source_root.rglob("*.rs")) if source_root.exists() else []:
        tokens = rust_tokens(path.read_text(encoding="utf-8"))
        relative = path.relative_to(workspace_root).as_posix()
        issues = import_issues(tokens)
        for offset, line, literal in compiled_path_issues(tokens):
            if literal is None:
                issues.append((offset, line, "include! path is not a string literal"))
                continue
            resolved = (path.parent / literal).resolve()
            if not path_is_within(resolved, resolved_crate_root):
                issues.append(
                    (
                        offset,
                        line,
                        f"compiled source path escapes crates/wirt: {literal}",
                    )
                )
        for _, line, message in sorted(issues):
            violations.append(f"{relative}:{line}: {message}")
    return violations


def violations(workspace_root: Path) -> list[str]:
    return dependency_violations(workspace_root) + source_violations(workspace_root)


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    found = violations(root)
    for violation in found:
        print(violation)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
