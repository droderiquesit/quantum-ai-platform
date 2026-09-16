#!/usr/bin/env python3
"""Re-run the register's own evidence commands and report the claims that have gone stale.

docs/DELIVERY-STATUS.md scores every blueprint section against the code, and each
verdict is backed by a runnable command rather than a line number, precisely so a
claim can be checked instead of believed. Nothing was checking them.

Four false claims were found by hand on 2026-09-16, and every one of them
understated what is built:

  §45.1  "returns nothing" for a resource declared two days earlier
  §5.7   "mandates exist nowhere as a type" — six declarations exist
  §43.3  "roughly twenty of thirty-five" — the row counted rows, not types
  §43.4  "`grep 'fn explain'` returns nothing" — four real definitions

The understating direction is the one nobody catches. An over-claim is found by
the next person who needs the thing and cannot find it; an under-claim just sits
there, and the cost is a lane rebuilding something that already exists. One did,
this week, and was stopped one tool call short.

This checks the half that is mechanically checkable: a command whose own
following prose asserts it returns nothing, which now returns something. It
cannot check whether a verdict is right, only whether the evidence under it still
says what the row claims. That is a floor, not a measurement.

Exits 1 when it finds candidates, so it can gate a review step — but it is not\nwired into `make check`, deliberately. Some of what it surfaces is correct as\nwritten, and a gate that blocks on a judgement call teaches people to bypass it.
"""

import re
import subprocess
import sys
from pathlib import Path

REGISTER = Path(__file__).resolve().parent.parent / "docs" / "DELIVERY-STATUS.md"

# Phrases that assert the command immediately before them produced no output.
# Anchored to the start of the text following the command, and bounded, so that a
# claim about some *later* command in the same cell is not attributed to this one.
# Only UNQUALIFIED emptiness: the claim that the command produced no output at all.
#
# "finds no production caller" and "has no consumer" are deliberately NOT here.
# They are claims about what the output *contains* — the command is expected to
# return test hits and doc comments, and the row is asserting something about
# them. Treating those as emptiness claims made the first version of this tool
# report 28 stale rows of which most were correct as written, and a checker that
# is wrong four times out of five trains its reader to skip it. That is a worse
# failure than not checking, because the one real finding arrives wearing the
# same colour as the noise.
EMPTINESS = re.compile(
    r"^[^`]{0,70}?"
    r"((returns?|finds?|prints?|gives?|yields?|matches) (nothing|zero|none)\b"
    r"|is empty\b|are empty\b|nothing at all\b|no hits\b|exits? 1\b)",
    re.IGNORECASE,
)

# A recursive search with no path argument walks the whole tree, including
# target/ and .git/. The register never means that; it is an extraction artefact
# from a cell that carries several commands.
# A row that has already been corrected quotes the false claim in order to record
# it: "this row said `grep ...` returns nothing, and it does not." Without this,
# the checker flags the correction as the defect — an instrument that reads its
# own output, which is the failure mode this repository's rules name explicitly
# about recount commands that match the prose quoting them. Looked for in the text
# BEFORE the command, because that is where a correction announces itself.
CORRECTED = re.compile(
    r"(said|claimed|until 20\d\d|correction|corrected|was wrong|no longer|struck through"
    r"|stopped being true|had returned)",
    re.IGNORECASE,
)

UNBOUNDED = re.compile(r"^(grep|rg)\b(?=.*\s-\w*r)(?!.*\s(backend|frontend|infrastructure|docs|scripts|\.github)\b)")

# Only commands whose whole job is to find things. A command that computes a count
# legitimately prints "0", which is output, and would read as a violation.
SEARCHER = re.compile(r"^(grep|ls|sed|awk|find|rg)\b")

# A command substitution or a redirect could do anything; this tool runs what the
# register says verbatim, so it declines rather than guessing.
UNSAFE = re.compile(r"\$\(|`|>|\brm\b|\bmv\b|\bdd\b")


def claims(text):
    """Yield (section, verdict, command) for every emptiness claim in the register."""
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        parts = line.split("|")
        if len(parts) < 5:
            continue
        section = parts[1].strip()
        verdict = parts[3].strip().strip("`")
        evidence = "|".join(parts[4:])
        for match in re.finditer(r"`([^`]+)`", evidence):
            # The register escapes pipes inside commands so the table still parses.
            command = match.group(1).replace("\\|", "|").strip()
            if not SEARCHER.match(command):
                continue
            if not EMPTINESS.match(evidence[match.end():]):
                continue
            if CORRECTED.search(evidence[max(0, match.start() - 160):match.start()]):
                continue
            yield section, verdict, command


def main():
    text = REGISTER.read_text()
    checked = skipped = 0
    violations = []

    for section, verdict, command in claims(text):
        if UNSAFE.search(command) or UNBOUNDED.match(command):
            skipped += 1
            continue
        checked += 1
        try:
            done = subprocess.run(
                ["bash", "-c", command],
                capture_output=True,
                text=True,
                timeout=30,
                cwd=REGISTER.parent.parent,
            )
        except subprocess.TimeoutExpired:
            violations.append((section, verdict, command, "timed out after 30s"))
            continue
        if done.stdout.strip():
            head = "\n".join(done.stdout.strip().splitlines()[:3])
            violations.append((section, verdict, command, head))

    for section, verdict, command, output in violations:
        print(f"CANDIDATE  §{section} ({verdict})")
        print(f"       claimed empty: {command}")
        for line in output.splitlines():
            print(f"       > {line}")
        print()

    print(f"checked {checked} emptiness claims, skipped {skipped} as unsafe to run, "
          f"{len(violations)} candidates")
    if violations:
        print()
        print("These are candidates for reading, not verdicts. The checker cannot tell a row")
        print("that is wrong from one whose command finds only doc comments while the row")
        print("means \"no code\" — that distinction needs a person. What it can tell you is")
        print("which claims are worth re-reading, and the four false ones found by hand on")
        print("2026-09-16 were all in this set.")
        print()
        print("When one is genuinely wrong, correct the row with a command that runs. Never")
        print("add a parenthetical explaining the output away: a command whose output needs")
        print("excusing is not evidence, and that is exactly how §5.7 stayed wrong for weeks.")
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
