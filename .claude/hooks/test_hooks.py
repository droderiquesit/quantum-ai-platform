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

import importlib.util
import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
GUARD = [sys.executable, str(HERE / "guard-dangerous-command.py")]
FORMAT = [sys.executable, str(HERE / "format-rust-after-edit.py")]

# Assembled so this file does not trip the guard when a shell passes it around.
TF = "terraform "
PUSH = "git " + "push "
FORCE = "--" + "force"

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
    ("git push origin main", 2, "a direct push to main is refused"),
    (
        "git push origin HEAD:ccr-0c1bacf8-kla0dd",
        2,
        "a direct push to the integration branch is refused",
    ),
    (
        "git push -u origin refs/heads/main",
        2,
        "a fully qualified push to main is refused",
    ),
    (
        "git push -u origin lane/L001-b-main-x",
        0,
        "a lane branch whose name contains main is allowed",
    ),
    ("git push origin domain", 0, "a branch merely ending in main is allowed"),
    (
        "git push -u origin lane/L052-b-GOV-007 && git log",
        0,
        "pushing a lane branch then running more commands is allowed",
    ),
]

# The gaps a review found on 2026-10-06: each is a force or a delete spelled
# so that substring matching did not see it. These run twice -- through the
# hook end to end, and against push_verdict alone -- because several are also
# caught by an older rule (a lease force contains the "--force" substring, a
# ":main" deletion matches PROTECTED_PUSH), and a case two rules catch cannot
# tell you that one of them has stopped working.
PUSH_CASES: list[tuple[str, int, str]] = [
    (PUSH + "origin +HEAD:main", 2, "a '+' refspec force to main is refused"),
    (PUSH + "origin +lane/x", 2, "a '+' refspec force to a lane is refused"),
    (PUSH + "origin lane/x +lane/y", 2, "a '+' on a later refspec is refused"),
    (PUSH + "origin lane/x " + "-f", 2, "a trailing short force flag is refused"),
    (PUSH + "-f", 2, "a short force flag at end of command is refused"),
    (PUSH + "-u" + "f origin lane/x", 2, "a force bundled into -uf is refused"),
    (PUSH + "-f" + "u origin lane/x", 2, "a force bundled into -fu is refused"),
    (PUSH + "origin lane/x " + FORCE + "-with-lease", 2, "lease force refused"),
    (
        PUSH + "origin lane/x " + FORCE + "-with-lease=lane/x:abc123",
        2,
        "a lease force with an expected value is refused",
    ),
    (PUSH + "origin lane/x " + FORCE + "-if-includes", 2, "if-includes refused"),
    (PUSH + "origin lane/x --forc", 2, "an abbreviated force option is refused"),
    (PUSH + "origin --delete lane/x", 2, "a --delete of a lane is refused"),
    (PUSH + "origin -d lane/x", 2, "a -d delete of a lane is refused"),
    (PUSH + "origin --delete main", 2, "a --delete of main is refused"),
    (PUSH + "origin :main", 2, "the ':main' deletion form is refused"),
    (
        PUSH + "origin :ccr-0c1bacf8-kla0dd",
        2,
        "the ':' deletion of the integration branch is refused",
    ),
    (PUSH + "origin :lane/x", 2, "the ':lane/x' deletion form is refused"),
    (PUSH + "origin --mirror", 2, "a mirror push is refused"),
    (PUSH + "--mirror origin", 2, "a mirror push with the flag first is refused"),
    (PUSH + "origin --prune", 2, "a pruning push is refused"),
    (
        "git -C /tmp/wt " + PUSH + "origin +lane/x",
        2,
        "a force behind git's own -C option is refused",
    ),
    (
        "cargo test && " + PUSH + "origin :lane/x",
        2,
        "a deletion after another command is refused",
    ),
    (
        "cat > d.md <<'EOF'\n" + PUSH + "origin +HEAD:main\nEOF",
        0,
        "a heredoc quoting a refspec force is allowed",
    ),
    # Allow cases: the parser must not mistake these for a force or delete.
    (PUSH + "-u origin lane/L001-b-x", 0, "an upstream push of a lane is allowed"),
    (PUSH + "origin feature-f", 0, "a branch whose name ends in -f is allowed"),
    (PUSH + "origin feature-d", 0, "a branch whose name ends in -d is allowed"),
    (PUSH + "origin lane/x:lane/x", 0, "an explicit src:dst refspec is allowed"),
    (PUSH + "-u origin HEAD", 0, "pushing HEAD to its own name is allowed"),
    (PUSH + "--dry-run origin lane/x", 0, "a dry run is allowed"),
    (PUSH + "-o ci.skip origin lane/x", 0, "a push option is allowed"),
    (PUSH + "--follow-tags origin lane/x", 0, "following tags is allowed"),
    (
        "git commit -m 'drop -f and the +x refspec' && git log -1",
        0,
        "a commit message quoting a flag is not a push flag",
    ),
    (
        'bash -c "' + PUSH + 'origin +lane/x"',
        2,
        "a force inside a quoted bash -c script is refused",
    ),
    ("git log --oneline -f", 0, "a -f on a command other than push is allowed"),
]


def load_guard():
    spec = importlib.util.spec_from_file_location(
        "guard", HERE / "guard-dangerous-command.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(argv: list[str], payload: object) -> int:
    return subprocess.run(
        argv, input=json.dumps(payload), capture_output=True, text=True
    ).returncode


def main() -> int:
    failures = 0

    for command, expected, name in GUARD_CASES + PUSH_CASES:
        got = run(GUARD, {"tool_input": {"command": command}})
        if got != expected:
            failures += 1
            print(f"FAIL  {name}: expected exit {expected}, got {got}")
        else:
            print(f"ok    {name}")

    guard = load_guard()
    for command, expected, name in PUSH_CASES:
        verdict = guard.push_verdict(guard.strip_heredocs(command))
        got = 0 if verdict is None else 2
        if got != expected:
            failures += 1
            print(f"FAIL  push parser alone, {name}: expected {expected}, got {got}")
        else:
            print(f"ok    push parser alone: {name}")

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
