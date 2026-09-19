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

Two claim classes are mechanically checkable and both are checked here.

  EMPTINESS   a command whose own following prose asserts it returns nothing,
              which now returns something.

  ONLY-TESTS  a command the row expects to return lines, where the prose
              asserts every one of them is under `tests/` or behind
              `#[cfg(test)]`. Added 2026-09-19 after a lane found four false
              rows of exactly this shape that the emptiness check could never
              see, all four false in the understating direction again.

The second class needs a rule the first does not, and the rule is the whole
reason it can be made precise: **position decides**. A hit in `src/` *after*
that file's last top-level `#[cfg(test)]` is a test, and several rows in this
register turn on nothing else. A checker that classified by path alone would
call every `src/` hit production and report a row false whenever its type
happened to be exercised in a unit-test module beside the code — which is
most of them.

Three kinds of hit are deliberately not counted against a row, because each
would produce noise the docstring below already argues is fatal:

  * a hit whose line is a comment. The register's rows mean code. A row saying
    "only tests" that is contradicted by a doc comment mentioning the name is
    correct as written, and the first version of this tool learned that lesson
    on a different claim class.
  * a hit on a declaration — `fn`, `struct`, `impl` and the rest. "No
    production caller" is a claim about callers, and the definition is not one
    of them. Flagging the definition would make the tool fire on every row in
    this class, which is the same as not firing at all.
  * output that is not `path:line:text`. A command run without `-n` cannot be
    positioned, so the `#[cfg(test)]` rule cannot be applied to it. Such a row
    is counted as unpositionable and skipped, never guessed at. Guessing is
    how a checker earns the reputation that makes its real findings invisible.

It cannot check whether a verdict is right, only whether the evidence under it
still says what the row claims. That is a floor, not a measurement.

Exits 1 when it finds candidates, so it can gate a review step — but it is not
wired into `make check`, deliberately. Some of what it surfaces is correct as
written, and a gate that blocks on a judgement call teaches people to bypass it.
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

# The second class: the row expects lines and asserts they are all tests.
#
# Narrow on purpose. Every alternative below names *tests* or *a production
# caller* explicitly; nothing here matches a bare "no consumer" or "nothing
# reads it", which are claims about the whole workspace rather than about the
# output of the command beside them, and which this tool cannot settle.
ONLY_TESTS = re.compile(
    r"^[^`]{0,70}?"
    r"((returns?|finds?|matches|prints?|gives?|yields?) only (the |its own |two )*tests?\b"
    r"|(returns?|finds?|matches|prints?|gives?|yields?) only test \w+"
    r"|only (a |the )?test (callers?|hits?|text|sites?|code|references?)\b"
    r"|(is|are) (constructed|built|called|used|reached|exercised|invoked) only (in|by|from)"
    r" (its own )?tests?\b"
    r"|only (in|under|from) `?tests/`?"
    r"|(every|each) (hit|line|match|caller) is (a |under )?tests?\b"
    r"|(is|are) (both )?test.only\b"
    r"|no non.test (caller|reference|consumer)s?\b"
    r"|(finds?|has|have) no production caller\b)",
    re.IGNORECASE,
)

# `path:line:text`, which is what a positioned grep prints and the only shape
# the `#[cfg(test)]` rule can be applied to.
HIT = re.compile(r"^(?P<path>[^:]+):(?P<line>\d+):(?P<text>.*)$")

# A line of prose inside the source. The register's rows mean code.
COMMENT = re.compile(r"^\s*(//|/\*|\*(?!/))")

# A definition rather than a use. "No production caller" is a claim about
# callers; the thing's own declaration is never one, and counting it would make
# this check fire on every row in the class.
#
# A closing brace on the same line disqualifies it, and that clause was put
# here by the self-test rather than by reasoning. `fn caller() { foo(); }`
# matches every declaration pattern there is and is a call site — the one
# shape this check exists to find. A signature this repository's rustfmt emits
# never closes its own body, so "contains `}`" separates a declaration from a
# one-line function that calls something.
DECLARATION = re.compile(
    r"^\s*(pub(\s*\([^)]*\))?\s+)?"
    r"(async\s+|const\s+|unsafe\s+|extern\s+\S+\s+|default\s+)*"
    r"(fn|struct|enum|trait|type|union|mod|impl|static|macro_rules!)\b"
)

# Paths whose every line is test code whatever its position in the file.
TEST_PATH = re.compile(r"(^|/)(tests|benches|examples)/")

UNBOUNDED = re.compile(r"^(grep|rg)\b(?=.*\s-\w*r)(?!.*\s(backend|frontend|infrastructure|docs|scripts|\.github)\b)")

# Only commands whose whole job is to find things. A command that computes a count
# legitimately prints "0", which is output, and would read as a violation.
SEARCHER = re.compile(r"^(grep|ls|sed|awk|find|rg)\b")

# A command substitution or a redirect could do anything; this tool runs what the
# register says verbatim, so it declines rather than guessing.
UNSAFE = re.compile(r"\$\(|`|>|\brm\b|\bmv\b|\bdd\b")


def unescape_table_pipes(command):
    """Undo the register's markdown-table pipe escaping WITHOUT breaking grep patterns.

    A markdown table cell must escape a literal `|`, so a shell pipe is written
    `\\|` in the register. But `\\|` is ALSO how BRE spells alternation, and
    `grep -in 'bigtable\\|alloydb'` means one thing to grep and another to a
    naive unescaper. Replacing every `\\|` with `|` turns that pattern into a
    search for the literal text `bigtable|alloydb`, which occurs in no tree.

    That is not a cosmetic bug. The command then returns nothing, the checker
    reads nothing as "the claim holds", and the row is confirmed whatever the
    repository contains. 62 of the register's 115 emptiness claims carry a
    pattern-internal pipe, so the first version of this script could not fire
    over a majority of its own denominator while reporting them all as checked
    — the `MaxExpectedShortfall` shape, in the tool written to catch it. It is
    why §17.2 sat false and unflagged: read as grep reads it, its
    proof-of-absence command returns 37 lines.

    The discriminator is quoting, not spacing. Inside a quoted string the pipe
    belongs to the pattern and the backslash is grep's; outside quotes it is
    the shell's pipe, escaped only so the table parses. Spacing looks tempting
    and is wrong: `'a\\|b'` and `x \\| y` differ by quoting reliably and by
    whitespace only by convention.
    """
    out = []
    quote = None
    index = 0
    while index < len(command):
        char = command[index]
        if quote:
            if char == quote:
                quote = None
            if char == "\\" and command[index + 1: index + 2] == "|":
                # Inside quotes: grep's alternation. Keep the backslash.
                out.append("\\|")
                index += 2
                continue
        elif char in ("'", '"'):
            quote = char
        elif char == "\\" and command[index + 1: index + 2] == "|":
            # Outside quotes: a shell pipe the table forced us to escape.
            out.append("|")
            index += 2
            continue
        out.append(char)
        index += 1
    return "".join(out)


_CFG_TEST_CACHE = {}


def last_top_level_cfg_test(path):
    """Line number of the file's last unindented `#[cfg(test)]`, or None.

    Unindented on purpose. A `#[cfg(test)]` nested inside a module gates only
    that module, and everything after it in the file is production again; the
    last one at column zero is the point past which the rest of the file is
    test code in every crate in this workspace. Reading it as "anywhere" would
    classify production code below a nested test module as a test, which is
    the error that matters here — it makes an under-claim look confirmed.
    """
    key = str(path)
    if key not in _CFG_TEST_CACHE:
        try:
            lines = path.read_text(errors="replace").splitlines()
        except OSError:
            _CFG_TEST_CACHE[key] = None
            return None
        last = None
        for number, line in enumerate(lines, 1):
            if line.startswith("#[cfg(test)]"):
                last = number
        _CFG_TEST_CACHE[key] = last
    return _CFG_TEST_CACHE[key]


def classify_hit(root, hit):
    """One of test / comment / declaration / production / unpositionable."""
    match = HIT.match(hit)
    if not match:
        return "unpositionable"
    path = match.group("path")
    text = match.group("text")
    if TEST_PATH.search(path):
        return "test"
    if COMMENT.match(text):
        return "comment"
    source = root / path
    if not source.is_file():
        return "unpositionable"
    # Position before shape: a declaration inside a test module is a test, and
    # saying so keeps the two buckets meaning what they are named.
    cut = last_top_level_cfg_test(source)
    if cut is not None and int(match.group("line")) > cut:
        return "test"
    # An empty body is still a declaration, so `{}` is removed before looking
    # for the brace that would mean this line closes a body it also filled.
    if DECLARATION.match(text) and "}" not in text.replace("{}", ""):
        return "declaration"
    return "production"


def claims(text):
    """Yield (kind, section, verdict, command) for every checkable claim.

    `kind` is "empty" or "only-tests". A command may legitimately carry only
    one: the two phrasings are mutually exclusive by construction, because a
    row asserting the output is empty is not also asserting what is in it.
    """
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
            command = unescape_table_pipes(match.group(1)).strip()
            if not SEARCHER.match(command):
                continue
            following = evidence[match.end():]
            if EMPTINESS.match(following):
                kind = "empty"
            elif ONLY_TESTS.match(following):
                kind = "only-tests"
            else:
                continue
            if CORRECTED.search(evidence[max(0, match.start() - 160):match.start()]):
                continue
            yield kind, section, verdict, command


SELF_TEST_SOURCES = {
    "crates/a/src/lib.rs": (
        "pub fn install_mirror() {}\n"          # 1  declaration
        "fn caller() { install_mirror(); }\n"   # 2  production use
        "/// install_mirror is described here\n"  # 3  comment
        "#[cfg(test)]\n"                        # 4  the cut
        "mod tests {\n"
        "    fn t() { install_mirror(); }\n"    # 6  past the cut, a test
        "}\n"
    ),
    "crates/a/src/nested.rs": (
        "mod inner {\n"
        "    #[cfg(test)]\n"                    # 2  indented: gates only `inner`
        "    mod t { fn x() { install_mirror(); } }\n"
        "}\n"
        "fn after() { install_mirror(); }\n"    # 5  production, below a nested cut
    ),
    "crates/a/tests/it.rs": "fn t() { install_mirror(); }\n",  # 1  test by path
}


def self_test():
    """Prove the classifier draws the distinctions the register turns on.

    Run with `--self-test`. Every case here is one the tool would otherwise
    get wrong silently: a declaration read as a caller, a doc comment read as
    code, a nested `#[cfg(test)]` read as though it ended the file, or a
    positioned test hit read as production. Each of those would make an
    under-claiming row look confirmed, which is the direction nobody catches.
    """
    import tempfile

    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, wanted {want!r}")

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        for name, body in SELF_TEST_SOURCES.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        _CFG_TEST_CACHE.clear()

        cases = [
            ("declaration", "crates/a/src/lib.rs:1:pub fn install_mirror() {}", "declaration"),
            ("production use", "crates/a/src/lib.rs:2:fn caller() { install_mirror(); }",
             "production"),
            ("doc comment", "crates/a/src/lib.rs:3:/// install_mirror is described here",
             "comment"),
            ("past the cut", "crates/a/src/lib.rs:6:    fn t() { install_mirror(); }", "test"),
            ("tests/ path", "crates/a/tests/it.rs:1:fn t() { install_mirror(); }", "test"),
            ("below a nested cut", "crates/a/src/nested.rs:5:fn after() { install_mirror(); }",
             "production"),
            ("no line number", "crates/a/src/lib.rs:pub fn install_mirror", "unpositionable"),
            ("absent file", "crates/gone/src/lib.rs:2:install_mirror();", "unpositionable"),
        ]
        for name, hit, want in cases:
            check(name, classify_hit(root, hit), want)

    phrases = [
        ("finds only test callers, so path 3", True),
        ("matches only test text, never a retention floor", True),
        ("still has only a test caller: `grep", True),
        ("finds no production caller, because no withdrawal", True),
        ("are both test-only, so the whole venue profile", True),
        # Not this class: emptiness, and claims about the whole workspace that
        # the output of one command cannot settle.
        ("returns nothing", False),
        ("finds no consumer outside the crate", False),
        ("shows one production caller, the panel's `CausalAnalyst`", False),
        ("is tested at `ls backend/crates`", False),
    ]
    for phrase, want in phrases:
        check(f"phrase {phrase!r}", bool(ONLY_TESTS.match(phrase)), want)

    for line in failures:
        print(f"SELF-TEST FAIL  {line}")
    total = len(cases) + len(phrases)
    print(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


def main():
    if "--self-test" in sys.argv[1:]:
        return self_test()

    text = REGISTER.read_text()
    root = REGISTER.parent.parent
    checked = {"empty": 0, "only-tests": 0}
    skipped = 0
    unpositionable = 0
    violations = []

    for kind, section, verdict, command in claims(text):
        if UNSAFE.search(command) or UNBOUNDED.match(command):
            skipped += 1
            continue
        try:
            done = subprocess.run(
                ["bash", "-c", command],
                capture_output=True,
                text=True,
                timeout=30,
                cwd=root,
            )
        except subprocess.TimeoutExpired:
            checked[kind] += 1
            violations.append((kind, section, verdict, command, "timed out after 30s"))
            continue

        output = done.stdout.strip()
        if kind == "empty":
            checked[kind] += 1
            if output:
                head = "\n".join(output.splitlines()[:3])
                violations.append((kind, section, verdict, command, head))
            continue

        # only-tests. An empty result says nothing about a claim that every
        # returned line is a test, so it is not a finding either way.
        if not output:
            continue
        lines = output.splitlines()
        verdicts = [classify_hit(root, line) for line in lines]
        if "unpositionable" in verdicts:
            # No line numbers, or a path that is not in this tree. The
            # `#[cfg(test)]` rule cannot be applied, so the row is left alone
            # rather than judged on the half of the evidence that parsed.
            unpositionable += 1
            continue
        checked[kind] += 1
        offenders = [
            line for line, what in zip(lines, verdicts) if what == "production"
        ]
        if offenders:
            head = "\n".join(offenders[:3])
            if len(offenders) > 3:
                head += f"\n... and {len(offenders) - 3} more of {len(lines)} lines"
            violations.append((kind, section, verdict, command, head))

    for kind, section, verdict, command, output in violations:
        claimed = "claimed empty" if kind == "empty" else "claimed tests only"
        print(f"CANDIDATE  §{section} ({verdict}) [{kind}]")
        print(f"       {claimed}: {command}")
        for line in output.splitlines():
            print(f"       > {line}")
        print()

    print(f"checked {checked['empty']} emptiness claims and {checked['only-tests']} "
          f"tests-only claims, skipped {skipped} as unsafe to run and "
          f"{unpositionable} as unpositionable, {len(violations)} candidates")
    if violations:
        print()
        print("These are candidates for reading, not verdicts. The checker cannot tell a row")
        print("that is wrong from one whose command finds only doc comments while the row")
        print("means \"no code\" — that distinction needs a person. What it can tell you is")
        print("which claims are worth re-reading, and the four false ones found by hand on")
        print("2026-09-16 were all in this set.")
        print()
        print("A tests-only candidate names the lines it could not account for. Read those")
        print("lines, not the count: the check excludes comments and declarations already,")
        print("so what is printed is a use, in a src/ file, at a position ahead of that")
        print("file's last top-level #[cfg(test)]. That is a production reference unless the")
        print("file does something unusual — and if it does, the row should say which.")
        print()
        print("When one is genuinely wrong, correct the row with a command that runs. Never")
        print("add a parenthetical explaining the output away: a command whose output needs")
        print("excusing is not evidence, and that is exactly how §5.7 stayed wrong for weeks.")
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
