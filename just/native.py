"""Native desktop runtime discovery shared by Flutter and Rust packagers."""
from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess


def run(*args: str, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def find_windows_crt() -> Path:
    candidates = []
    if os.environ.get("VCToolsRedistDir"):
        candidates.extend(Path(os.environ["VCToolsRedistDir"]).glob("x64/Microsoft.VC*.CRT"))
    vswhere = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
    if vswhere.is_file():
        installation = run(str(vswhere), "-latest", "-products", "*", "-requires",
                           "Microsoft.VisualStudio.Component.VC.Redist.14.Latest", "-property",
                           "installationPath", stdout=subprocess.PIPE, text=True).stdout.strip()
        if installation:
            candidates.extend((Path(installation) / "VC/Redist/MSVC").glob("*/x64/Microsoft.VC*.CRT"))
    required = ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll")
    valid = [p for p in candidates if all((p / name).is_file() for name in required)]
    if not valid:
        raise ValueError("Install the Visual Studio C++ x64 redistributable build component")
    return max(valid, key=lambda p: tuple(int(part) for part in re.findall(r"\d+", p.parents[1].name)))

