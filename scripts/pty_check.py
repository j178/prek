#!/usr/bin/env python
# Copyright (c) 2023 Axo Developer Co.
#
# Permission is hereby granted, free of charge, to any
# person obtaining a copy of this software and associated
# documentation files (the "Software"), to deal in the
# Software without restriction, including without
# limitation the rights to use, copy, modify, merge,
# publish, distribute, sublicense, and/or sell copies of
# the Software, and to permit persons to whom the Software
# is furnished to do so, subject to the following
# conditions:
#
# The above copyright notice and this permission notice
# shall be included in all copies or substantial portions
# of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
# ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
# TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
# PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
# SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
# CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
# OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
# IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
# DEALINGS IN THE SOFTWARE.
# /// script
# requires-python = ">=3.10"
# dependencies = [
#   "rich>=12.4.4",
# ]
# ///
#
# This prek hook is here to tests all the terminal properties
# and allows a little dog-fooding of the terminal emulator changes.
# You can use it to test the processes run by prek as hooks (particularly
# python hook in this case) to see all the properties of the stdin/stdout/stderr.
# whether they are TTY, their size, and the environment variables
# that indicate the terminal emulator and its properties.
# It also detects the parent processes to determine the terminal emulator
# and any multiplexers like tmux or screen.
# It prints the collected information in a structured JSON format.
# This is useful for debugging terminal-related issues in scripts or applications.
#
# Usage (make sure your prek is installed from the current sources)
#
#    prek run pty-check --hook-stage manual
#
# You can also change stages for the pty-check hook in .pre-commit-config.yaml to
# the `[pre-commit, manual] - and you will see the terminal information when you run git commit.
# This is important because `prek run` passes the parent terminal to the hook, but when you run git commit,
# there is no terminal available at all.
#
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys

from rich.console import Console
console = Console(width=400, color_system="standard")

TERMINAL_ENV_PATTERNS = (
    "TERM", "COLORTERM", "TTY", "TMUX", "STY", "SCREEN", "KONSOLE", "ITERM",
    "WT_", "VTE", "XTERM", "GNOME_TERMINAL", "ALACRITTY", "WEZTERM", "WARP",
    "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE",
)
TERMINAL_ENV_EXTRAS = {
    "SHELL",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "SSH_TTY",
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "WT_PROFILE_ID",
    "KONSOLE_VERSION",
    "KONSOLE_PROFILE_NAME",
    "ITERM_SESSION_ID",
}

EMULATOR_HINTS = {
    "iTerm": "iTerm2",
    "Apple_Terminal": "Apple Terminal",
    "Terminal": "Apple Terminal",
    "WezTerm": "WezTerm",
    "wezterm-gui": "WezTerm",
    "kitty": "kitty",
    "alacritty": "Alacritty",
    "Hyper": "Hyper",
    "ghostty": "Ghostty",
    "WarpTerminal": "Warp",
    "warp": "Warp",
    "Code Helper": "VS Code Terminal",
    "code": "VS Code Terminal",
    "gnome-terminal": "GNOME Terminal",
    "konsole": "Konsole",
    "xfce4-terminal": "Xfce Terminal",
    "tilix": "Tilix",
    "xterm": "xterm",
    "urxvt": "urxvt",
    "rxvt": "rxvt",
    "terminator": "Terminator",
    "idea": "IntelliJ IDEA Terminal",
    "IntelliJ": "IntelliJ IDEA Terminal",
    "jetbrains-idea": "IntelliJ IDEA Terminal",
    # Multiplexers (reported separately)
    "tmux": "tmux",
    "screen": "GNU screen",
}


def collect_terminal_env(environ: dict[str, str]) -> dict[str, str]:
    def is_terminal_key(k: str) -> bool:
        if k in TERMINAL_ENV_EXTRAS:
            return True
        return any(p in k for p in TERMINAL_ENV_PATTERNS)

    result = {k: v for k, v in environ.items() if is_terminal_key(k)}
    # Sort keys for stable output
    return dict(sorted(result.items(), key=lambda kv: kv[0].lower()))


def fd_info(fd: int) -> dict[str, str | None]:
    info: dict[str, str | None] = {
        "isatty": None,
        "ttyname": None,
        "cols": None,
        "rows": None,
    }
    try:
        is_tty = os.isatty(fd)
        info["isatty"] = bool(is_tty)
        if is_tty:
            try:
                info["ttyname"] = os.ttyname(fd)
            except OSError:
                # Fallback to `tty` command
                try:
                    cp = subprocess.run(
                        ["tty"], capture_output=True, text=True, check=False
                    )
                    name = cp.stdout.strip()
                    info["ttyname"] = name if name and name != "not a tty" else None
                except Exception:
                    info["ttyname"] = None
        try:
            size = os.get_terminal_size(fd)
            info["cols"] = size.columns
            info["rows"] = size.lines
        except OSError:
            size = shutil.get_terminal_size(fallback=(0, 0))
            info["cols"] = size.columns or None
            info["rows"] = size.lines or None
    except Exception:
        pass
    return info


def get_parent_comm_and_ppid(pid: int) -> tuple[int, str] | None:
    # Returns (ppid, comm) for the given pid, or None
    try:
        cp = subprocess.run(
            ["ps", "-p", str(pid), "-o", "ppid=", "-o", "comm="],
            capture_output=True,
            text=True,
            check=False,
        )
        if cp.returncode != 0:
            return None
        line = cp.stdout.strip().splitlines()
        if not line:
            return None
        # Format: "<ppid> <comm>"
        parts = line[-1].strip().split(None, 1)
        if not parts:
            return None
        ppid = int(parts[0])
        comm = parts[1] if len(parts) > 1 else ""
        return ppid, comm.strip()
    except Exception:
        return None


def get_parent_process_chain() -> list[dict[str, str]]:
    chain: list[dict[str, str]] = []
    pid = os.getpid()
    # Include current process first
    p = get_parent_comm_and_ppid(pid)
    # If ps can't fetch current, still proceed to parents via os.getppid()
    if p:
        chain.append({"pid": str(pid), "comm": p[1] or ""})
        pid = p[0]
    else:
        pid = os.getppid()

    # Walk up until root
    visited = set()
    while pid and pid not in visited and pid not in (0, 1):
        visited.add(pid)
        res = get_parent_comm_and_ppid(pid)
        if not res:
            break
        ppid, comm = res
        chain.append({"pid": str(pid), "comm": comm or ""})
        pid = ppid

    return chain  # from child to ancestors


def detect_terminal_from_chain(chain: list[dict[str, str]]) -> dict[str, str | None]:
    names = [c.get("comm", "") for c in chain]
    names_lower = [n.lower() for n in names]

    def match_hint(skip_multiplexers: bool) -> str | None:
        for key, label in EMULATOR_HINTS.items():
            k = key.lower()
            # optionally skip multiplexers for emulator detection
            if skip_multiplexers and k in ("tmux", "screen"):
                continue
            for n in names_lower:
                if k in n:
                    return label
        return None

    in_tmux = any("tmux" in n for n in names_lower) or bool(os.environ.get("TMUX"))
    in_screen = any("screen" in n for n in names_lower) or bool(os.environ.get("STY"))

    emulator = match_hint(skip_multiplexers=True)
    multiplexer = "tmux" if in_tmux else ("GNU screen" if in_screen else None)

    return {
        "emulator": emulator,
        "multiplexer": multiplexer,
    }


def main() -> None:
    data = {
        "python": {
            "version": platform.python_version(),
            "executable": sys.executable,
            "platform": platform.platform(),
        },
        "tty": {
            "stdin": fd_info(0),
            "stdout": fd_info(1),
            "stderr": fd_info(2),
        },
        "env": collect_terminal_env(os.environ),
        "process": {
            "chain": get_parent_process_chain(),  # child -> ancestors
        },
    }
    data["process"].update(detect_terminal_from_chain(data["process"]["chain"]))

    # Common quick fields up top for convenience
    data["summary"] = {
        "TERM": os.environ.get("TERM"),
        "TERM_PROGRAM": os.environ.get("TERM_PROGRAM"),
        "TERM_PROGRAM_VERSION": os.environ.get("TERM_PROGRAM_VERSION"),
        "SHELL": os.environ.get("SHELL"),
        "inside_tmux": bool(os.environ.get("TMUX")),
        "inside_screen": bool(os.environ.get("STY")),
        "emulator_detected": data["process"].get("emulator"),
        "multiplexer_detected": data["process"].get("multiplexer"),
    }

    console.print(json.dumps(data, indent=2, sort_keys=False))


if __name__ == "__main__":
    main()
