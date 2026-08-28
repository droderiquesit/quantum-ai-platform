#!/usr/bin/env python3
"""Format a Rust file the moment it is written.

A PostToolUse hook rather than a rule, because ``rustfmt`` is deterministic:
running it is strictly better than asking a model to remember to. Formatting
never becomes a separate review round, and ``cargo fmt --all --check`` in CI
stops being the thing that fails a run for whitespace.

It exits 0 unconditionally. A formatting failure must never block an edit --
the edit is the work and the formatting is a convenience, and a hook that
refuses a write because ``rustfmt`` is missing would make the repository
unusable on a machine without the toolchain.

Written in Python for the same reason as its sibling guard: the shell version
of that hook was broken by a quoting bug that made *every* command fail, and
neither hook is worth that risk.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0

    path = payload.get("tool_input", {}).get("file_path", "")
    if not isinstance(path, str) or not path.endswith(".rs"):
        return 0

    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        return 0

    try:
        subprocess.run(
            [rustfmt, "--edition", "2024", path],
            check=False,
            capture_output=True,
            timeout=20,
        )
    except Exception:
        # Deliberately swallowed. See the module docstring: a formatting
        # problem must not cost the user their edit.
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
