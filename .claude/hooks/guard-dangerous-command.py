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
import shlex
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

# Branches that change only through a reviewed pull request. With plain
# pushes allowed for worker sessions (so a lane can publish its own branch),
# nothing else stopped one from pushing straight to these: on 2026-10-06 a
# lane session pushed five unreviewed commits onto the integration branch
# minutes after pushes were allowed. A name is matched as a whole refspec
# target -- after whitespace or a colon, and followed by the end of the
# argument -- so "lane/L001-b-main-x" or "domain" is not mistaken for main.
PROTECTED_PUSH = re.compile(
    r"\bgit\s+push\b[^\n;&|]*?[\s:](?:refs/heads/)?(main|ccr-0c1bacf8-kla0dd)(?=$|[\s;&|])",
    re.M,
)

PROTECTED_BRANCHES = ("main", "ccr-0c1bacf8-kla0dd")

# The substring rules above and PROTECTED_PUSH see text, and git sees argv. On
# 2026-10-06 a review listed the pushes that text matching let through, every
# one of them a force or a delete by another spelling: a refspec with a
# leading "+" (git's per-ref force), "-f" as the last word with no trailing
# space, "-f" bundled into "-uf", --force-with-lease and --force-if-includes
# (no "--force" substring problem there, but "--force" was the only spelling
# anyone had written down), --delete and "-d", the ":branch" deletion form,
# and --mirror, which force-updates and deletes every remote ref at once. So
# a push is now read the way git reads it -- tokenised, options parsed, each
# refspec examined -- and anything that can rewrite or remove a ref somebody
# else has pulled is refused, whatever branch it names.
#
# Long options are matched as git matches them, by unambiguous prefix:
# "--forc" is --force to git, and a guard that only knows full spellings is a
# guard with a documented bypass.
PUSH_LONG_REFUSED: dict[str, tuple[str, bool]] = {
    # option: (what was refused, whether it deletes rather than forces)
    "force": ("a force push", False),
    "force-with-lease": ("a force push (--force-with-lease still rewrites)", False),
    "force-if-includes": ("a force push (--force-if-includes)", False),
    "delete": ("a remote branch deletion", True),
    "mirror": ("a mirror push, which force-updates and deletes remote refs", True),
    "prune": ("a push that deletes remote branches (--prune)", True),
}
PUSH_SHORT_REFUSED: dict[str, tuple[str, bool]] = {
    "f": ("a force push", False),
    "d": ("a remote branch deletion", True),
}
# Push options whose value may follow as a separate word; that word is the
# option's argument, not a repository or a refspec.
PUSH_LONG_WITH_ARG = ("repo", "receive-pack", "exec", "push-option")
PUSH_SHORT_WITH_ARG = "o"
# Git's own global options that consume the next word, before the subcommand.
GIT_GLOBAL_WITH_ARG = (
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--super-prefix",
)

FORCE_ADVICE = (
    "A force push rewrites history other checkouts have already pulled. "
    "Merge the base branch instead; on a branch you created alone, ask the "
    "user first."
)
DELETE_ADVICE = (
    "Deleting a remote branch is irreversible for every other checkout that "
    "tracks it, and on main or the integration branch it removes the shared "
    "history outright. Ask the user, naming the branch."
)
SEPARATORS = {";", "&", "&&", "|", "||", "(", ")", ";;", "|&"}

HEREDOC = re.compile(r"""<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1""")


def segments(text: str) -> list[list[str]]:
    """Split shell text into simple commands, each a list of words.

    Quoting is honoured so that a commit message mentioning a flag is one word
    and not a flag. When the text will not tokenise (an unbalanced quote), it
    falls back to whitespace splitting: over-reading a malformed command can
    only refuse it, and a shell would refuse it too.
    """
    out: list[list[str]] = []
    for line in text.replace("\\\n", " ").split("\n"):
        try:
            lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
            lexer.whitespace_split = True
            lexer.commenters = ""
            words = list(lexer)
        except ValueError:
            words = line.split()
        current: list[str] = []
        for word in words:
            if word in SEPARATORS:
                if current:
                    out.append(current)
                current = []
            else:
                current.append(word)
        if current:
            out.append(current)
    return out


def push_arguments(words: list[str]) -> list[str] | None:
    """The words after ``push`` if this simple command is a git push."""
    for start, word in enumerate(words):
        if word.rsplit("/", 1)[-1] != "git":
            continue
        i = start + 1
        while i < len(words):
            token = words[i]
            if token in GIT_GLOBAL_WITH_ARG:
                i += 2
            elif token.startswith("-"):
                i += 1
            else:
                break
        if i < len(words) and words[i] == "push":
            return words[i + 1 :]
    return None


def refuse_push(args: list[str]) -> tuple[str, str] | None:
    """Return (what was refused, what to do instead) for a dangerous push."""
    positional: list[str] = []
    i = 0
    options_done = False
    while i < len(args):
        token = args[i]
        i += 1
        if options_done or token == "-" or not token.startswith("-"):
            positional.append(token)
            continue
        if token == "--":
            options_done = True
            continue
        if token.startswith("--"):
            name, has_value, _ = token[2:].partition("=")
            if len(name) >= 2:
                for option, (refused, deletes) in PUSH_LONG_REFUSED.items():
                    if option.startswith(name):
                        return refused, DELETE_ADVICE if deletes else FORCE_ADVICE
                if not has_value and any(
                    option.startswith(name) for option in PUSH_LONG_WITH_ARG
                ):
                    i += 1
            continue
        # A bundle of short flags, "-uf" being "-u -f". A flag that takes an
        # argument ends the bundle: the rest of the word, or the next word,
        # is its value.
        for position, flag in enumerate(token[1:]):
            if flag in PUSH_SHORT_REFUSED:
                refused, deletes = PUSH_SHORT_REFUSED[flag]
                return refused, DELETE_ADVICE if deletes else FORCE_ADVICE
            if flag in PUSH_SHORT_WITH_ARG:
                if position == len(token) - 2:
                    i += 1
                break

    # The first positional word is the repository; the rest are refspecs.
    for refspec in positional[1:]:
        if refspec == ":":
            return (
                "a push of every matching branch, the shared branches included",
                "Name the one branch you mean to push.",
            )
        if refspec.startswith("+"):
            return "a force push (a '+' refspec forces that ref)", FORCE_ADVICE
        if refspec.startswith(":"):
            target = refspec[1:].removeprefix("refs/heads/")
            shared = " of a shared branch" if target in PROTECTED_BRANCHES else ""
            return f"a remote branch deletion{shared} ({refspec})", DELETE_ADVICE
    return None


def push_verdict(text: str, depth: int = 0) -> tuple[str, str] | None:
    """The first dangerous push in ``text``, looking inside quoted scripts.

    ``bash -c "git push ..."`` hands the whole push to the shell as one quoted
    word, which the tokeniser rightly keeps whole -- so a word that itself
    contains a push is parsed again as a command line, to a bounded depth.
    """
    for words in segments(text):
        args = push_arguments(words)
        if args is not None:
            verdict = refuse_push(args)
            if verdict is not None:
                return verdict
        if depth < 3:
            for word in words:
                if "push" in word and any(c.isspace() for c in word):
                    verdict = push_verdict(word, depth + 1)
                    if verdict is not None:
                        return verdict
    return None


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

    protected = PROTECTED_PUSH.search(inspected)
    if protected is not None:
        sys.stderr.write(
            "Refused by .claude/hooks/guard-dangerous-command.py: a direct push "
            f"to the shared branch {protected.group(1)}\n\n"
            "Push your own branch and open a pull request against it; the "
            "orchestrator reviews and merges.\n"
        )
        return 2

    verdict = push_verdict(inspected)
    if verdict is not None:
        refused, instead = verdict
        sys.stderr.write(
            "Refused by .claude/hooks/guard-dangerous-command.py: "
            f"{refused}\n\n{instead}\n"
        )
        return 2

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
