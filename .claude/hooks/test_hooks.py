#!/usr/bin/env python3
"""Regression tests for the hooks. Run: python3 .claude/hooks/test_hooks.py

These exist because both hooks have already been wrong in ways that were
invisible until exercised. The guard blocked a heredoc that merely quoted a
dangerous command, and an earlier shell version had a quoting bug that made it
fail on every command -- blocking all work rather than the dangerous subset.

Each case names the property. The allow cases matter as much as the block
cases: a guard that refuses everything is not secure, it is broken, and only
the allow cases can tell the two apart.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
GUARD = [sys.executable, str(HERE / "guard-dangerous-command.py")]
FORMAT = [sys.executable, str(HERE / "format-rust-after-edit.py")]

# Assembled so this file does not trip the guard when a shell passes it around.
TF = "terraform "

GUARD_CASES: list[tuple[str, int, str]] = [
    ("cargo test --workspace", 0, "an ordinary test run is allowed"),
    ("git push -u origin feature", 0, "an ordinary push is allowed"),
    ("rm -rf target/debug", 0, "a delete inside the repository is allowed"),
    (TF + "plan -out=tf.plan", 0, "a plan is allowed"),
    ("git push origin main --force", 2, "a force push is refused"),
    ("git push -f origin main ", 2, "a short-flag force push is refused"),
    ("rm -rf /etc", 2, "a delete rooted outside the repository is refused"),
    (TF + "destroy", 2, "a teardown is refused"),
    (TF + "apply " + "-auto-approve", 2, "an unreviewed apply is refused"),
    ("kubectl delete pod x", 2, "a cluster deletion is refused"),
    ("gcloud compute instances delete vm", 2, "a cloud deletion is refused"),
    ("git clean -xdf", 2, "discarding untracked work is refused"),
    ("psql -c 'DROP TABLE users'", 2, "a destructive statement is refused"),
    (
        "cat > doc.md <<EOF\nnever run rm -rf / here\nEOF",
        0,
        "a heredoc documenting a dangerous command is allowed",
    ),
    (
        "cat > d.md <<'EOF'\n" + TF + "destroy\nEOF",
        0,
        "a quoted heredoc documenting a dangerous command is allowed",
    ),
    (
        "cat > d.md <<EOF\nsafe\nEOF\nrm -rf /etc",
        2,
        "a real command after a heredoc is still refused",
    ),
    ("", 0, "an empty command is allowed"),
]


def run(argv: list[str], payload: object) -> int:
    return subprocess.run(
        argv, input=json.dumps(payload), capture_output=True, text=True
    ).returncode


def main() -> int:
    failures = 0

    for command, expected, name in GUARD_CASES:
        got = run(GUARD, {"tool_input": {"command": command}})
        if got != expected:
            failures += 1
            print(f"FAIL  {name}: expected exit {expected}, got {got}")
        else:
            print(f"ok    {name}")

    # Malformed input must never block: the payload shape is not this hook's
    # to validate, and refusing on it would stop every call the day it changes.
    for payload in ["not json", {}, {"tool_input": {}}, {"tool_input": {"command": 7}}]:
        raw = payload if isinstance(payload, str) else json.dumps(payload)
        got = subprocess.run(
            GUARD, input=raw, capture_output=True, text=True
        ).returncode
        if got != 0:
            failures += 1
            print(f"FAIL  malformed payload {raw!r} was refused (exit {got})")
    print("ok    malformed payloads are allowed through")

    # The formatter must never block an edit, whatever it is handed.
    for payload in [
        {"tool_input": {"file_path": "/nonexistent/x.rs"}},
        {"tool_input": {"file_path": "notes.md"}},
        {"tool_input": {}},
    ]:
        got = run(FORMAT, payload)
        if got != 0:
            failures += 1
            print(f"FAIL  formatter returned {got} for {payload}")
    print("ok    the formatter never blocks an edit")

    print(f"\n{'FAILED' if failures else 'all hook tests pass'} ({failures} failures)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
