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

An emptiness candidate is reported in one of two buckets, and the split was
added 2026-09-19 after a run produced 25 undifferentiated candidates of which
eight found nothing but prose:

  CANDIDATE   at least one returned line is code. The lines printed are the
              code ones, not the head of the output — a grep walks its tree
              alphabetically, so the old behaviour showed three doc comments
              for a row whose one real hit sat in a file late in the walk, and
              that reads as noise.

  PROSE-ONLY  every returned line is a comment. The command does return
              output, so the row's wording is stale as written, and that is
              worth printing; but four of the eight were §37.1/§37.2/§37.3 and
              §38.4 citing `grep -rn capital_fabric_file
              infrastructure/environments/*/terraform.tfvars`, whose every hit
              is a commented-out assignment and whose own next clause says so:
              "and each tfvars says why". Prose-only rows do not set the exit
              code. A gate that fires on a row explaining its own output is the
              fastest way to teach a reader that this tool can be ignored.

Two things make that split possible and neither was here before. `#` opens a
comment in Terraform, YAML, shell and Python and opens an attribute in Rust,
so the comment rule is chosen by file extension rather than tried both ways —
`#[cfg(test)]` read as prose would silently disable the positioning rule this
whole tool rests on. And `grep -n` against a single named file prints
`line:text` with no path, which the old hit pattern could not read at all.

Supersession is the other correction of the same day. `CORRECTED` looks at the
text *before* a command, which catches a row that announces its correction and
then quotes the false claim; the common order is the opposite, with the
original sentence first and the amendment appended after it. §40.5 opens with
a `returns nothing` that is no longer true and then says so itself, in the
same cell, and this tool reported it. A retraction now counts when it follows
the command *and* re-cites a distinctive token of it — not merely when the
cell contains an amendment somewhere, because an amendment is exactly when the
older claims beside it are most likely to have gone stale too.

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

# Words too generic to prove that a later retraction is about *this* command.
# `terraform` appears in half the register's paths; a retraction mentioning it
# says nothing about which claim it retracts.
GENERIC_TOKENS = {
    "infrastructure",
    "terraform",
    "backend",
    "frontend",
    "crates",
    "include",
    "scripts",
    "workflows",
    "environments",
    # Layer names. These are path segments in half the register's commands and
    # in half its prose; a retraction that happens to quote a directory is not
    # a retraction of a claim about what is inside it.
    "services",
    "runtime",
}

# A token specific enough to tie a retraction to a command: an identifier or a
# path segment, eight characters or more, that is not on the list above.
COMMAND_TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_]{7,}")


def superseded(evidence, command, end):
    """Whether a later clause in the same cell retracts this command's claim.

    `CORRECTED` looks at the text *before* a command, which catches a row that
    announces its own correction and then quotes the false claim. It does not
    catch the other order, and the other order is the common one: the cell
    carries the original sentence, and the amendment is appended after it.

    §40.5 is the worked example and it cost this tool a false report. The row
    opens "`grep -rn 'google_compute_security_policy\\|...'` returns nothing,
    so Cloud Armor, the Global HTTPS LB and Cloud CDN are all absent", and
    then says, in the same cell, "**Amended 2026-09-14 (ADR 0069).** `grep
    google_compute_security_policy infrastructure/terraform` no longer returns
    nothing". The row is correct and self-correcting, and the checker reported
    it as stale.

    The retraction has to be tied to *this* command rather than to any
    amendment anywhere in the cell, or an appended note about one claim would
    excuse every older claim beside it — and an amendment is exactly when the
    older claims beside it are most likely to have gone stale too. So a
    retraction counts only when it re-cites a distinctive token of the command
    it retracts, within a window of it.
    """
    later = evidence[end:]
    tokens = [
        token
        for token in dict.fromkeys(COMMAND_TOKEN.findall(command))
        if token.lower() not in GENERIC_TOKENS
    ]
    if not tokens:
        return False
    for mark in CORRECTED.finditer(later):
        window = later[max(0, mark.start() - 200): mark.end() + 200]
        if any(token in window for token in tokens):
            return True
    return False


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

# `line:text`, which is what `grep -n` prints when it was given exactly one
# file. The register does this often — §1.3's evidence names one source file —
# and the path is knowable only from the command, so the `#[cfg(test)]` rule
# still cannot be applied. What *can* be applied is the comment rule, and
# that is the only thing the emptiness class asks of a line.
BARE_HIT = re.compile(r"^(?P<line>\d+):(?P<text>.*)$")


def hit_text(hit):
    """(path, text) for either grep shape, or (None, None) when neither fits.

    `(None, None)` is not "no comment"; it is "cannot tell", and the caller
    treats it as code. Every place this is used, guessing prose would delete a
    finding and guessing code merely prints one.
    """
    match = HIT.match(hit)
    if match:
        return match.group("path"), match.group("text")
    match = BARE_HIT.match(hit)
    if match:
        return None, match.group("text")
    return None, None

# A line of prose inside the source. The register's rows mean code.
#
# Two spellings, chosen by the file's extension rather than tried together,
# because `#` is a comment in Terraform and an attribute in Rust and guessing
# between them is the whole problem. `#[cfg(test)]` read as a comment would
# silently disable the positioning rule this tool is built on.
COMMENT = re.compile(r"^\s*(//|/\*|\*(?!/))")
HASH_COMMENT = re.compile(r"^\s*#")

# Where `#` opens a comment. Deliberately a list of suffixes rather than
# "anything that is not Rust": a file type nobody enumerated falls through to
# the slash rule and its `#` lines count as code, which over-reports rather
# than under-reports. Under-reporting is the direction that loses a finding.
HASH_COMMENT_SUFFIXES = (
    ".tf",
    ".tfvars",
    ".tftpl",
    ".hcl",
    ".yaml",
    ".yml",
    ".sh",
    ".bash",
    ".py",
    ".toml",
    ".cfg",
    ".ini",
    "Makefile",
    "Dockerfile",
)


def is_comment_line(path, text):
    """Whether `text`, read as a line of `path`, is prose rather than code.

    `path` may be None — a `grep -n` against one named file prints `line:text`
    with no path at all — in which case only the slash spelling is tried. That
    is the conservative side: a `#` line in an unattributable hit counts as
    code and gets reported.

    This distinction is worth its own function because it decided four of this
    tool's twenty-five candidates on the day it was written. §37.1, §37.2,
    §37.3 and §38.4 all cite `grep -rn capital_fabric_file
    infrastructure/environments/*/terraform.tfvars` and all say it "returns
    nothing"; every line it returns is a commented-out assignment, and each row
    goes on to say so in its own next clause — "and each tfvars says why". A
    checker that reports a row whose very next words explain its output is the
    checker that teaches its reader to skip it.
    """
    if path is not None and str(path).endswith(HASH_COMMENT_SUFFIXES):
        return bool(HASH_COMMENT.match(text))
    return bool(COMMENT.match(text))

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
    if is_comment_line(path, text):
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
            if superseded(evidence, command, match.end()):
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
    # `#` is a comment here and an attribute in Rust, which is why the two
    # spellings are chosen by extension rather than tried together.
    "environments/dev/terraform.tfvars": (
        "# capital_fabric_file = \"data/fabric/x.json\"\n"     # 1  comment
        "region = \"europe-west2\"\n"                          # 2  code
    ),
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

        # The comment rule on its own, which the emptiness class uses without
        # going through `classify_hit` at all. Every case here is one that
        # decided a real candidate on 2026-09-19.
        comment_cases = [
            # A commented-out assignment in a tfvars: four rows turn on this.
            ("tfvars hash", "environments/dev/terraform.tfvars",
             "# capital_fabric_file = \"x\"", True),
            ("tfvars code", "environments/dev/terraform.tfvars", "region = \"x\"", False),
            # The one that must never flip: a Rust attribute is not prose, and
            # reading it as prose would disable the positioning rule.
            ("rust attribute", "crates/a/src/lib.rs", "#[cfg(test)]", False),
            ("rust doc comment", "crates/a/src/lib.rs", "/// a note", True),
            ("rust module comment", "crates/a/src/lib.rs", "//! a note", True),
            ("terraform resource", "infrastructure/terraform/main.tf",
             "resource \"google_compute_security_policy\" \"edge\" {", False),
            ("terraform hash", "infrastructure/terraform/main.tf", "  # why it is unset", True),
            # No path: the slash rule only, and `#` counts as code rather than
            # being guessed at. Over-reporting is the safe direction.
            ("pathless comment", None, "    /// a note", True),
            ("pathless hash", None, "# not attributable", False),
        ]
        for name, path, text, want in comment_cases:
            check(f"comment {name}", is_comment_line(path, text), want)

        # And the two grep shapes, because a `grep -n` against one named file
        # prints no path and the emptiness class still has to read its text.
        shape_cases = [
            ("positioned", "src/a.rs:12:    foo();", ("src/a.rs", "    foo();")),
            ("bare line", "82:    pub funding_rate_annual_f64: f64,",
             (None, "    pub funding_rate_annual_f64: f64,")),
            ("neither", "Binary file target/x matches", (None, None)),
        ]
        for name, hit, want in shape_cases:
            check(f"shape {name}", hit_text(hit), want)

    # A retraction appended after the command it retracts. §40.5's real text,
    # trimmed: without this the row reads as stale while it is correcting
    # itself, and with a rule that ignored the token check, the unrelated
    # claim beside it would be excused too.
    amended = (
        "The public edge does not: `grep -rn 'google_compute_security_policy' "
        "infrastructure/terraform --include=*.tf` returns nothing, so Cloud Armor is absent. "
        "**Amended 2026-09-14 (ADR 0069).** `grep google_compute_security_policy "
        "infrastructure/terraform` no longer returns nothing: `modules/public-edge/` declares it."
    )
    command = "grep -rn 'google_compute_security_policy' infrastructure/terraform --include=*.tf"
    end = amended.index("returns nothing")
    check("retraction after the command", superseded(amended, command, end), True)

    unrelated = (
        "`grep -rn 'some_other_symbol' backend/crates` returns nothing. "
        "**Amended 2026-09-14.** `grep google_compute_security_policy infrastructure/terraform` "
        "no longer returns nothing."
    )
    check(
        "amendment about a different command",
        superseded(unrelated, "grep -rn 'some_other_symbol' backend/crates", 40),
        False,
    )
    check(
        "generic tokens alone do not excuse a claim",
        superseded(
            "`ls infrastructure/terraform` returns nothing. **Amended.** terraform no longer.",
            "ls infrastructure/terraform",
            30,
        ),
        False,
    )

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
    total = len(cases) + len(phrases) + len(comment_cases) + len(shape_cases) + 3
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
    prose_only = []

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
            if not output:
                continue
            # Every line, not the first three. The old version printed the
            # head of the output and the head of a grep's output is
            # alphabetical, so a row whose one real hit sat in a file late in
            # the walk was shown as three doc comments and read as noise.
            lines = output.splitlines()
            code = [
                line
                for line in lines
                if not is_comment_line(*hit_text(line))
                or hit_text(line) == (None, None)
            ]
            head = "\n".join(code[:3] if code else lines[:3])
            remainder = (len(code) if code else len(lines)) - 3
            if remainder > 0:
                head += f"\n... and {remainder} more"
            if code:
                if len(code) < len(lines):
                    head += f"\n({len(lines) - len(code)} further lines are comments)"
                violations.append((kind, section, verdict, command, head))
            else:
                # Every line is prose. The command does return output, so the
                # row's wording is stale as written, and that is worth saying —
                # but nothing it found is code, and reporting it beside a real
                # finding is what makes the real finding invisible.
                prose_only.append((kind, section, verdict, command, head))
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

    for _, section, verdict, command, output in prose_only:
        print(f"PROSE-ONLY §{section} ({verdict})")
        print(f"       claimed empty: {command}")
        for line in output.splitlines():
            print(f"       > {line}")
        print()

    print(f"checked {checked['empty']} emptiness claims and {checked['only-tests']} "
          f"tests-only claims, skipped {skipped} as unsafe to run and "
          f"{unpositionable} as unpositionable, {len(violations)} candidates "
          f"and {len(prose_only)} prose-only")
    if prose_only and not violations:
        print()
        print("A PROSE-ONLY row's command does return lines, so its wording is stale as")
        print("written, but every line it returns is a comment. Several such rows are correct")
        print("in substance and say so in their own next clause — §37.1's goes on to write")
        print("\"and each tfvars says why\" about the very lines printed above it. Read them")
        print("after the candidates, not instead of them.")
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
    # Prose-only rows do not set the exit code. They are reported so nothing is
    # hidden, and they are not a gate, because a gate that fires on a row whose
    # own next clause explains the output is the fastest way to teach a reader
    # that this tool can be ignored.
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
