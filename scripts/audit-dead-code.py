#!/usr/bin/env python3
"""Find public items in `src/` that nothing outside their own declaration mentions.

This repository's standard is that a type, control or metric nothing reads is not
built — `MaxExpectedShortfall` shipped in every default limit set and could never
fire, because nothing filled the state it read, and the rules keep it as the
example of what not to ship. The register scores rows on whether a production
caller exists. Neither the compiler nor clippy enforces that for a `pub` item:
`pub` is an escape hatch from dead-code analysis, so an unreached public function
is invisible to every gate this workspace runs.

What this reports is a CANDIDATE list, not a defect list. A `pub` item with no
caller is one of three things and only a person can tell them apart:

  * genuine dead surface — build it a caller or delete it;
  * a deliberate public API a downstream crate is expected to use, in which case
    nothing is wrong and the workspace is simply the only consumer today;
  * a control that was built and never wired, which is the case worth finding
    and the reason this script exists.

The third is what the register calls UNREACHED, and several rows have turned on
exactly it this session: `Scoreboard` scored regimes nothing consulted,
`degraded_models` marked models nothing read, `default_slos()` shipped eight
objectives with no caller at all.

Heuristics and their limits, stated so nobody over-reads the output:

  * Matching is by NAME, workspace-wide. A short or common name (`new`, `id`,
    `len`) will be referenced by something unrelated and so never reported — this
    under-reports, deliberately, because a false positive here sends somebody to
    delete working code.
  * Trait method implementations are matched by the method name, so a trait whose
    impls are called through the trait object reads as referenced.
  * Macro-generated call sites are invisible.
  * `#[cfg(test)]` position decides what counts as production: a `src/` hit after
    a file's first unindented `#[cfg(test)]` is a test and is excluded from
    declarations, but references from anywhere count — so an item called ONLY by
    tests is NOT reported here. That is a separate claim class; use
    `scripts/audit-register.py`, which checks it.

Exits 1 when it finds candidates, so a reviewer can gate on it. It is not wired
into `make check`: most of what it surfaces is legitimate public API, and a gate
that fires on a judgement call teaches people to bypass it.
"""

import collections
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRATES = ROOT / "backend" / "crates"

DECL = re.compile(
    r"\s*pub (?:async )?(?:const )?(fn|struct|enum|trait) ([A-Za-z_][A-Za-z0-9_]*)"
)


def production_declarations():
    """Map each pub item name to the (location, kind) pairs that declare it."""
    found = collections.defaultdict(list)
    for path in CRATES.rglob("*.rs"):
        text = str(path)
        if "/target/" in text or "/tests/" in text:
            continue
        lines = path.read_text(errors="replace").split("\n")
        # The first unindented #[cfg(test)] starts the test module; a declaration
        # after it is a test fixture and is not production surface.
        cut = len(lines)
        for index, line in enumerate(lines):
            if line.startswith("#[cfg(test)]"):
                cut = index
                break
        for index, line in enumerate(lines[:cut]):
            match = DECL.match(line)
            if match:
                rel = path.relative_to(ROOT)
                found[match.group(2)].append((f"{rel}:{index + 1}", match.group(1)))
    return found


def unreferenced(declarations):
    """Names whose every workspace occurrence is one of their own declarations."""
    names = list(declarations)
    orphans = []
    # Batched because one alternation over several thousand names is slower than
    # the process spawns it saves.
    for start in range(0, len(names), 400):
        batch = names[start : start + 400]
        pattern = "|".join(re.escape(name) + r"\b" for name in batch)
        done = subprocess.run(
            ["grep", "-rhoE", pattern, "--include=*.rs", str(CRATES)],
            capture_output=True,
            text=True,
        )
        seen = collections.Counter(line for line in done.stdout.split("\n") if line)
        for name in batch:
            if seen.get(name, 0) <= len(declarations[name]):
                location, kind = declarations[name][0]
                orphans.append((location, kind, name))
    return sorted(orphans)


def main():
    declarations = production_declarations()
    orphans = unreferenced(declarations)

    for location, kind, name in orphans:
        print(f"{kind:7s} {name:40s} {location}")

    print()
    print(
        f"{len(declarations)} public items declared in src/, "
        f"{len(orphans)} with no reference outside their own declaration"
    )
    if orphans:
        print()
        print("Each is a candidate, not a defect. Decide which of three it is: dead")
        print("surface to delete, deliberate API with no consumer yet, or a control")
        print("that was built and never wired. Only the third is a defect, and it is")
        print("the one this repository keeps paying for — a control nothing reads")
        print("looks exactly like protection until somebody needs it to fire.")
    return 1 if orphans else 0


if __name__ == "__main__":
    sys.exit(main())
