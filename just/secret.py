r"""Per-user store for the CLI credentials (API keys, tokens) `just` recipes need.

One file outside every repository holds them: %APPDATA%\monodev\secrets.env on
Windows, $XDG_CONFIG_HOME/monodev/secrets.env (default ~/.config/monodev/) on
other systems. Each line is blank, a # comment or NAME=value, where the value is
everything after the first "=", taken verbatim: no quotes, escapes or inline
comments.

No command prints a value. Output names secrets by NAME, and errors cite line
numbers, never line text. `run` is the only way a value leaves the file: into
the environment of the one command it starts.

    python just/secret.py set NAME...      prompt for each value, or read one piped value
    python just/secret.py list             print the stored names
    python just/secret.py remove NAME...
    python just/secret.py import NAME...   copy user environment variables
    python just/secret.py run NAME... -- COMMAND [ARG...]
"""
from __future__ import annotations

import contextlib
from collections.abc import Callable, Mapping
import getpass
import inspect
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shutil
import subprocess
import sys
import tempfile

NAME = re.compile(r"[A-Z_][A-Z0-9_]*")
ENTRY = re.compile(rf"({NAME.pattern})=(.*)")
ACTIONS = ("set", "list", "remove", "import", "run")
BATCH_SPECIAL = '&|<>^%"\r\n'
USAGE = """\
usage: secret.py set NAME...      store values typed at a hidden prompt, or one piped value
       secret.py list             print the stored names
       secret.py remove NAME...   delete stored names
       secret.py import NAME...   copy user environment variables into the store
       secret.py run NAME... -- COMMAND [ARG...]
                                  run COMMAND with the named values in its environment"""


class SecretError(Exception):
    """A failure printed as `! item  message`, then `action` on its own line.

    Built from names, paths and line numbers only, so printing it never
    discloses a value.
    """

    def __init__(self, item: str, message: str, action: str | None = None, code: int = 1) -> None:
        super().__init__(f"{item}  {message}")
        self.item = item
        self.message = message
        self.action = action
        self.code = code


class RetryPrompt(Exception):
    """Input at a prompt that can't be a value; the prompt asks again."""


def store_path(platform: str, env: Mapping[str, str]) -> Path:
    """The store file for this user. `platform` is a sys.platform value.

    The folder must be absolute: a relative one would put the store inside
    whatever directory a command runs in, such as a repository. A relative
    XDG_CONFIG_HOME is ignored, as the XDG spec says.
    """
    if platform == "win32":
        variable, flavour, below = "APPDATA", PureWindowsPath, ()
    else:
        xdg = env.get("XDG_CONFIG_HOME", "")
        if xdg and PurePosixPath(xdg).is_absolute():
            return Path(xdg, "monodev", "secrets.env")
        variable, flavour, below = "HOME", PurePosixPath, (".config",)
    base = env.get(variable, "")
    if not base:
        raise SecretError("secret", f"{variable} is not set")
    if not flavour(base).is_absolute():
        raise SecretError("secret", f"{variable} is not an absolute path")
    return Path(base, *below, "monodev", "secrets.env")


def check_name(name: str) -> str:
    if not NAME.fullmatch(name):
        raise SecretError(name, "not a valid name: use A-Z, 0-9 and _, not starting with a digit", code=2)
    return name


class Store:
    """The store file's lines as read, so saving changes only the lines that
    were set or removed and keeps comments and order."""

    def __init__(self, path: Path, lines: list[str]) -> None:
        self.path = path
        self._lines = lines
        self._index: dict[str, int] = {}
        for number, line in enumerate(lines, start=1):
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            if "\0" in line:
                raise SecretError(str(path), f"line {number} holds a NUL character; fix or delete that line")
            entry = ENTRY.fullmatch(line)
            if entry is None:
                raise SecretError(str(path), f"line {number} is not NAME=value; fix or delete that line")
            if not entry.group(2).strip():
                raise SecretError(str(path), f"line {number} has no value after =; fix or delete that line")
            name = entry.group(1)
            if name in self._index:
                raise SecretError(str(path), f"line {number} repeats {name} from line "
                                             f"{self._index[name] + 1}; delete one of them")
            self._index[name] = number - 1

    @classmethod
    def load(cls, path: Path) -> Store:
        """Read the store; a missing file is an empty store. utf-8-sig and
        universal newlines accept a file saved by Notepad (BOM, CRLF)."""
        try:
            text = path.read_text(encoding="utf-8-sig")
        except FileNotFoundError:
            return cls(path, [])
        except UnicodeDecodeError:
            # The decode error quotes a byte of the file, which may be a value's.
            raise SecretError(str(path), "is not UTF-8 text; save it as UTF-8") from None
        lines = text.split("\n")
        if lines[-1] == "":
            lines.pop()
        return cls(path, lines)

    def names(self) -> list[str]:
        return list(self._index)

    def get(self, name: str) -> str | None:
        index = self._index.get(name)
        return None if index is None else self._lines[index].split("=", 1)[1]

    def put(self, name: str, value: str) -> bool:
        """Set a value; True when it replaced one. A line break would let a
        value write a second entry, so callers check values first and this
        refuses any that slips through."""
        check_name(name)
        if any(c in value for c in "\r\n\0"):
            raise ValueError(f"the value for {name} holds a line break or NUL")
        try:
            value.encode("utf-8")
        except UnicodeEncodeError:
            raise ValueError(f"the value for {name} can't be saved as UTF-8") from None
        line = f"{name}={value}"
        if name in self._index:
            self._lines[self._index[name]] = line
            return True
        self._index[name] = len(self._lines)
        self._lines.append(line)
        return False

    def delete(self, name: str) -> None:
        removed = self._index.pop(name)
        del self._lines[removed]
        self._index = {n: i - 1 if i > removed else i for n, i in self._index.items()}

    def save(self) -> None:
        """Write a temp file beside the store, then rename it over the store,
        so a crash leaves the old file or the new one. On POSIX the folder is
        0700 and the file 0600 (mkstemp's mode); on Windows both inherit the
        profile folder's permissions."""
        folder = self.path.parent
        folder.mkdir(parents=True, exist_ok=True)
        if sys.platform != "win32":
            os.chmod(folder, 0o700)
        handle, temp = tempfile.mkstemp(dir=folder, prefix=".secrets-", suffix=".tmp")
        try:
            with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as file:
                file.write("".join(line + "\n" for line in self._lines))
                file.flush()
                os.fsync(file.fileno())
            os.replace(temp, self.path)
        except BaseException:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(temp)
            raise


def check_value(name: str, value: str, source: str) -> str:
    """Refuse a value the store can't hold as one line. `source` says where it
    came from, for the error."""
    if "\n" in value or "\r" in value:
        raise SecretError(name, f"{source} holds {len(value.splitlines())} lines; a value is one line")
    if "\0" in value:
        raise SecretError(name, f"{source} holds a NUL character")
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        # A lone surrogate: a pasted emoji at the Windows prompt, or bytes
        # Python could not decode in a POSIX environment variable.
        raise SecretError(name, f"{source} holds text that can't be saved as UTF-8") from None
    return value


def stdin_is_terminal() -> bool:
    """Whether stdin is an interactive console that getpass can prompt on.

    On Windows isatty() is also True for the null device that Claude's tools
    attach, and getpass would then wait on a hidden console forever.
    GetConsoleMode succeeds only for a real console input handle.
    """
    stdin = sys.stdin
    if stdin is None:
        return False
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes
        import msvcrt
        try:
            handle = msvcrt.get_osfhandle(stdin.fileno())
        except (OSError, ValueError):
            return False
        get_console_mode = ctypes.WinDLL("kernel32", use_last_error=True).GetConsoleMode
        get_console_mode.argtypes = (wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
        get_console_mode.restype = wintypes.BOOL
        mode = wintypes.DWORD()
        return bool(get_console_mode(handle, ctypes.byref(mode)))
    try:
        return stdin.isatty()
    except ValueError:
        return False


def read_hidden_line(prompt: str, getwch: Callable[[], str], putwch: Callable[[str], None],
                     kbhit: Callable[[], bool]) -> str:
    """One line from the Windows console, echoing * per character.

    getpass's Windows reader stores the two codes an arrow or function key
    sends as part of the value, and returns at the first line of a paste,
    leaving the rest in the console for the shell to read and run after
    exit. Here such a key, or a paste of more than one line, drops whatever
    is waiting and raises RetryPrompt. U+00E0 (à) is also the prefix code of
    those keys, so a value holding it can't be typed here; pipe it in.
    """
    def end_line() -> None:
        putwch("\r")
        putwch("\n")

    def drain() -> str:
        waiting: list[str] = []
        while kbhit():
            waiting.append(getwch())
        return "".join(waiting)

    for char in prompt:
        putwch(char)
    typed: list[str] = []
    while True:
        char = getwch()
        if char in ("\r", "\n"):
            break
        if char == "\x03":
            raise KeyboardInterrupt
        if char in ("\x00", "\xe0"):
            getwch()  # the key's scan code
            end_line()
            drain()
            raise RetryPrompt("arrow and function keys can't edit a hidden value; enter it again")
        if char == "\b":
            if typed:
                typed.pop()
                for erase in "\b \b":
                    putwch(erase)
            continue
        typed.append(char)
        putwch("*")
    end_line()
    # A blank line leaves waiting keys alone: after a stray Enter, they are
    # the value for the prompt that follows.
    if typed and drain().strip():
        raise RetryPrompt("the pasted text holds more than one line; paste one line")
    return "".join(typed)


def hidden_input(prompt: str) -> str:
    """A line typed or pasted without showing it. getpass on POSIX, where
    restoring the terminal discards unread input; echoes * per character
    where supported (3.14+) so a paste shows that something arrived."""
    if sys.platform == "win32":
        import msvcrt
        return read_hidden_line(prompt, msvcrt.getwch, msvcrt.putwch, msvcrt.kbhit)
    if "echo_char" in inspect.signature(getpass.getpass).parameters:
        return getpass.getpass(prompt, echo_char="*")
    return getpass.getpass(prompt)


def prompt_value(name: str, ask: Callable[[str], str]) -> str:
    """Ask until the answer isn't blank: a stray Enter repeats the prompt
    instead of storing an empty value."""
    while True:
        try:
            value = ask(f"Value for {name}: ").strip()
        except RetryPrompt as retry:
            print(f"! {name}  {retry}", file=sys.stderr)
            continue
        if value:
            return check_value(name, value, "the typed value")


def piped_value(names: list[str], data: bytes) -> str:
    """The value piped to `set NAME`. Windows PowerShell 5.1 adds a BOM and
    both PowerShells add CRLF when piping to a program; both are dropped."""
    try:
        value = data.decode("utf-8-sig").strip()
    except UnicodeDecodeError:
        raise SecretError(names[0], "standard input is not UTF-8 text") from None
    if not value:
        raise SecretError("secret set", "no terminal and nothing on standard input",
                          action="Run in a terminal: just -g secret set " + " ".join(names))
    if len(names) != 1:
        raise SecretError("secret set", f"piped input sets one name; got {len(names)}", code=2)
    return check_value(names[0], value, "standard input")


def store_values(store: Store, values: dict[str, str]) -> int:
    replaced = {name: store.put(name, value) for name, value in values.items()}
    store.save()
    for name, was_replaced in replaced.items():
        print(f"✓ {name}  {'replaced' if was_replaced else 'stored'} in {store.path}")
    return 0


def cmd_set(store: Store, names: list[str], read_value: Callable[[str], str]) -> int:
    """Collect every value first, then write once, so an interrupted prompt
    stores nothing."""
    values: dict[str, str] = {}
    try:
        for name in names:
            values[name] = read_value(name)
    except (KeyboardInterrupt, EOFError):
        print(file=sys.stderr)  # end the prompt's line
        raise SecretError("secret set", "interrupted; nothing stored", code=130) from None
    return store_values(store, values)


def read_registry_value(subkey: str, name: str) -> str | None:
    """A string value under HKEY_CURRENT_USER\\<subkey>, with %VARIABLES%
    expanded for REG_EXPAND_SZ. None when the key or value is missing or
    isn't a string."""
    if sys.platform != "win32":
        return None
    import winreg
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, subkey) as key:
            value, kind = winreg.QueryValueEx(key, name)
    except FileNotFoundError:
        return None
    if kind == winreg.REG_EXPAND_SZ:
        return winreg.ExpandEnvironmentStrings(value)
    return value if kind == winreg.REG_SZ else None


def read_user_env(name: str) -> str | None:
    """A user environment variable as the user set it. On Windows that is
    HKCU\\Environment: a process started before the variable was set has no
    copy of it in its own environment."""
    if sys.platform == "win32":
        return read_registry_value("Environment", name)
    return os.environ.get(name)


def cmd_import(store: Store, names: list[str], read: Callable[[str], str | None]) -> int:
    """Copy user environment variables into the store, all or none. The
    variables stay set."""
    found = {name: read(name) for name in names}
    missing = [name for name, value in found.items() if value is None]
    if missing:
        raise SecretError(", ".join(missing), "no user environment variable by that name")
    values: dict[str, str] = {}
    for name, value in found.items():
        value = (value or "").strip()
        if not value:
            raise SecretError(name, "the user environment variable is empty")
        values[name] = check_value(name, value, "the user environment variable")
    return store_values(store, values)


def find_program(name: str, path: str | None) -> str | None:
    """shutil.which without Windows' search of the current directory.
    PowerShell, the recipes' shell, starts nothing from there without .\\,
    so `run` must not hand secrets to a program it found there."""
    key = "NoDefaultCurrentDirectoryInExePath"
    if sys.platform != "win32" or os.path.dirname(name) or key in os.environ:
        return shutil.which(name, path=path)
    os.environ[key] = "1"
    try:
        return shutil.which(name, path=path)
    finally:
        del os.environ[key]


def check_batch_arguments(program: str, args: list[str]) -> None:
    """cmd.exe parses the command line of a .cmd or .bat file: & starts a
    second command and %NAME% expands a variable, possibly a secret, into
    the command line. Refuse the characters it would act on."""
    suffix = Path(program).suffix.lower()  # which() takes the case from PATHEXT: .CMD
    if sys.platform != "win32" or suffix not in (".cmd", ".bat"):
        return
    for arg in args:
        for char in arg:
            if char in BATCH_SPECIAL:
                shown = "line break" if char in "\r\n" else char
                raise SecretError("secret run", f"{Path(program).stem}{suffix} is a batch file; cmd.exe would act "
                                                f"on the {shown} in an argument", code=2)


def cmd_run(store: Store | None, names: list[str], command: list[str], env: Mapping[str, str]) -> int:
    """Start `command` with the named secrets added to `env`; exit with its code.

    A name resolves to its store value, else to a non-empty value `env`
    already has, such as a token a CI job injects. The store wins when both
    have it, so a leftover variable can't shadow a rotated key. Other store
    entries stay out of the child's environment. Without a store, every
    name comes from `env`.
    """
    child_env = dict(env)
    missing: list[str] = []
    for name in names:
        value = store.get(name) if store is not None else None
        if value is not None:
            child_env[name] = value
        elif not env.get(name):
            missing.append(name)
    if missing:
        where = f"{store.path} or the environment" if store is not None else "the environment"
        raise SecretError("secret run", f"{', '.join(missing)} not in {where}",
                          action=f"Run: just -g secret set {' '.join(missing)}")
    # which() applies PATHEXT, so .cmd shims such as pnpm start on Windows.
    program = find_program(command[0], env.get("PATH"))
    if program is None:
        raise SecretError("secret run", f"{command[0]} not found on PATH", code=127)
    check_batch_arguments(program, command[1:])
    child = subprocess.Popen([program, *command[1:]], env=child_env)
    while True:
        try:
            code = child.wait()
            break
        except KeyboardInterrupt:
            continue  # the child got the same Ctrl+C; its exit code says how it ended
    # A child killed by signal N reports -N; shells report 128 + N.
    return 128 - code if code < 0 else code


def cmd_list(store: Store) -> int:
    names = store.names()
    if not names:
        print(f"0 secrets in {store.path}")
    for name in names:
        print(name)
    return 0


def cmd_remove(store: Store, names: list[str]) -> int:
    missing = [name for name in names if store.get(name) is None]
    if missing:
        raise SecretError(", ".join(missing), f"not stored in {store.path}")
    for name in names:
        store.delete(name)
    store.save()
    for name in names:
        print(f"✓ {name}  removed from {store.path}")
    return 0


def parse_names(action: str, args: list[str]) -> list[str]:
    """Validated names in the order given, without repeats."""
    if not args:
        raise SecretError(f"secret {action}", "needs at least one NAME", code=2)
    return [check_name(name) for name in dict.fromkeys(args)]


def split_command(args: list[str]) -> tuple[list[str], list[str]]:
    """`NAME... -- COMMAND [ARG...]` as (names, command)."""
    if "--" not in args:
        raise SecretError("secret run", "missing -- between the names and the command", code=2)
    split = args.index("--")
    if split == len(args) - 1:
        raise SecretError("secret run", "no command after --", code=2)
    return args[:split], args[split + 1:]


def run_action(action: str, args: list[str]) -> int:
    if action not in ACTIONS:
        raise SecretError("secret", f"unknown action {action}; use {', '.join(ACTIONS[:-1])} or {ACTIONS[-1]}",
                          code=2)
    if action == "list":
        if args:
            raise SecretError("secret list", "takes no names", code=2)
        return cmd_list(Store.load(store_path(sys.platform, os.environ)))
    command: list[str] = []
    if action == "run":
        args, command = split_command(args)
    names = parse_names(action, args)
    try:
        path = store_path(sys.platform, os.environ)
    except SecretError:
        # A CI job may have no home folder; names it injects need no store.
        if action == "run" and all(os.environ.get(name) for name in names):
            return cmd_run(None, names, command, os.environ)
        raise
    store = Store.load(path)
    if action == "set":
        if stdin_is_terminal():
            return cmd_set(store, names, lambda name: prompt_value(name, hidden_input))
        data = sys.stdin.buffer.read() if sys.stdin is not None else b""  # None: stdin closed
        value = piped_value(names, data)
        return cmd_set(store, names, lambda name: value)
    if action == "remove":
        return cmd_remove(store, names)
    if action == "import":
        return cmd_import(store, names, read_user_env)
    return cmd_run(store, names, command, os.environ)


def main(argv: list[str] | None = None) -> int:
    # A store path can hold characters a legacy console code page cannot
    # encode (this machine defaults to cp932); degrade the character, not
    # the command. Only a real text stream has reconfigure().
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            with contextlib.suppress(ValueError):
                reconfigure(errors="replace")
    args = sys.argv[1:] if argv is None else argv
    if args and args[0] in ("-h", "--help"):
        print(USAGE)
        return 0
    action, rest = (args[0], args[1:]) if args else ("list", [])
    try:
        return run_action(action, rest)
    except SecretError as error:
        print(f"! {error.item}  {error.message}", file=sys.stderr)
        if error.action:
            print(error.action, file=sys.stderr)
        return error.code
    except OSError as error:
        print(f"! secret  {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
