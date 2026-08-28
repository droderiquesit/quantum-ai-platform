#!/usr/bin/env python3
"""Refuse shell commands whose blast radius exceeds this repository.

A PreToolUse hook rather than a line in a rules file, because a security
boundary a model can reason its way past is not a boundary. Exit 2 blocks the
call and returns stderr to Claude; exit 0 allows it.

Scope is deliberately narrow. Every pattern below either destroys data no test
can recreate, rewrites history somebody else has already pulled, or touches
cloud resources this repository does not own. Ordinary destructive-looking
work -- ``rm`` inside ``target/``, ``git reset`` on an unpushed branch -- is
left alone, because a guard that fires constantly is a guard people route
around.

Two things this file learned the hard way, both during its own authoring:

* **Heredoc bodies must be stripped before matching.** The first version
  scanned the whole command string, so a heredoc *writing documentation about*
  a dangerous command was refused. It blocked the very file that explains why
  the command is blocked. A guard that cannot tell an instruction from a
  quotation of one costs more than it protects.

* **It is written in Python, not shell.** The shell version embedded a Python
  one-liner whose regex contained a single quote, which closed the enclosing
  shell quote and left the script syntactically invalid -- at which point the
  hook failed on *every* command, blocking all work rather than the dangerous
  subset. A guard that fails closed on its own bug is a denial of service
  against its own repository, so the logic lives somewhere it can be parsed and
  tested directly.

This is a guard against accident and inattention only. Anything with write
access to this file can delete it, which is why
``docs/claude/managed-policy-recommendations.md`` asks for the same rules in
managed policy, where a branch cannot reach them.
"""

from __future__ import annotations

import json
import re
import sys

# (needle(s), what was refused, what to do instead). A tuple of needles means
# every one of them must appear -- that is how "apply" is distinguished from
# "apply with -auto-approve".
RULES: list[tuple[tuple[str, ...], str, str]] = [
    (
        ("rm -rf /",),
        "a recursive delete rooted outside the repository",
        "Delete a specific path under the repository instead.",
    ),
    (
        ("rm -fr /",),
        "a recursive delete rooted outside the repository",
        "Delete a specific path under the repository instead.",
    ),
    (
        ("rm -rf ~",),
        "a recursive delete of the home directory",
        "Delete a specific path under the repository instead.",
    ),
    (
        ("git push", "--force"),
        "a force push",
        "A force push rewrites history other checkouts have already pulled. "
        "Merge the base branch instead; on a branch you created alone, ask "
        "the user first.",
    ),
    (
        ("git push", " -f "),
        "a force push",
        "A force push rewrites history other checkouts have already pulled. "
        "Merge the base branch instead.",
    ),
    (
        ("git reset --hard origin/",),
        "a command that discards uncommitted work",
        "Uncommitted work in this tree may belong to a parallel agent. Commit "
        "it to a WIP branch before discarding anything.",
    ),
    (
        ("git clean -", "d", "f"),
        "a command that discards untracked work",
        "Untracked files here may be a parallel agent's in-flight work. "
        "Commit them to a WIP branch before discarding anything.",
    ),
    (
        ("terraform destroy",),
        "an unapproved Terraform teardown",
        "Run a plan and show it to the user. Teardown goes through "
        ".github/workflows/infra.yml, which refuses prod.",
    ),
    (
        ("terraform apply", "-auto-approve"),
        "an unreviewed Terraform apply",
        "Run a plan and show it to the user before applying.",
    ),
    (
        ("gcloud ", " delete "),
        "a cloud resource deletion",
        "Deleting cloud resources is irreversible and may not be this "
        "repository's to delete. Ask the user, naming the exact resource.",
    ),
    (
        ("gsutil rm",),
        "a cloud storage deletion",
        "Ask the user, naming the exact object.",
    ),
    (
        ("kubectl delete",),
        "a cluster resource deletion",
        "Ask the user, naming the exact resource.",
    ),
    (
        ("DROP TABLE",),
        "a destructive database statement",
        "Write a reversible migration instead.",
    ),
    (
        ("DROP DATABASE",),
        "a destructive database statement",
        "Write a reversible migration instead.",
    ),
    (
        ("TRUNCATE ",),
        "a destructive database statement",
        "Write a reversible migration instead.",
    ),
]

HEREDOC = re.compile(r"""<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1""")


def strip_heredocs(text: str) -> str:
    """Remove every heredoc body, leaving only commands actually being run.

    Anything a heredoc carries is content being written to a file. Matching it
    would refuse a document that merely quotes a dangerous command, which is
    exactly what a rules file about dangerous commands has to do.
    """
    while True:
        match = HEREDOC.search(text)
        if match is None:
            return text
        rest = text[match.end() :]
        terminator = re.search(
            r"^\s*" + re.escape(match.group(2)) + r"\s*$", rest, re.M
        )
        if terminator is None:
            # An unterminated heredoc: everything after the marker is body.
            return text[: match.end()]
        text = text[: match.start()] + rest[terminator.end() :]


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except Exception:
        # Unparseable input is not evidence of a dangerous command, and
        # refusing on it would block every call the moment the payload shape
        # changed.
        return 0

    command = payload.get("tool_input", {}).get("command", "")
    if not isinstance(command, str) or not command.strip():
        return 0

    inspected = strip_heredocs(command)

    for needles, refused, instead in RULES:
        if all(needle in inspected for needle in needles):
            sys.stderr.write(
                "Refused by .claude/hooks/guard-dangerous-command.py: "
                f"{refused}\n\n{instead}\n"
            )
            return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
