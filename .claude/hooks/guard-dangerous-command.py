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
import os
import re
import shlex
import subprocess
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
# Except one thing, since the owner asked for a clean branch list: a branch
# deletion (--delete, -d, ":branch") is no longer refused by its spelling. It
# is checked instead -- see ``deletion_verdict`` -- and allowed only on origin,
# never for main or the integration branch, and only when the branch's tip is
# already in origin/main or held by an ``archive/*`` tag. --mirror and --prune
# delete refs nobody named, so they stay refused outright.
#
# Long options are matched as git matches them, by unambiguous prefix:
# "--forc" is --force to git, and a guard that only knows full spellings is a
# guard with a documented bypass.
PUSH_LONG_REFUSED: dict[str, tuple[str, bool]] = {
    # option: (what was refused, whether it deletes rather than forces)
    "force": ("a force push", False),
    "force-with-lease": ("a force push (--force-with-lease still rewrites)", False),
    "force-if-includes": ("a force push (--force-if-includes)", False),
    "mirror": ("a mirror push, which force-updates and deletes remote refs", True),
    "prune": ("a push that deletes remote branches (--prune)", True),
}
PUSH_SHORT_REFUSED: dict[str, tuple[str, bool]] = {
    "f": ("a force push", False),
}
# Deletion is the one push form that is no longer refused outright. It is
# collected rather than refused, and each named branch is then checked against
# the repository by ``deletion_verdict``.
PUSH_LONG_DELETE = "delete"
PUSH_SHORT_DELETE = "d"
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
PRESERVE_ADVICE = (
    "Merge it, or tag it `archive/{tag}` at its tip first "
    "(scripts/cleanup-branches.sh does both), then delete it."
)
# The remote whose tracking refs the check reads. A deletion naming any other
# remote, or a URL, is refused: refs/remotes/origin/* says nothing about it.
DELETION_REMOTE = "origin"
GIT_TIMEOUT_SECONDS = 5
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


def push_invocation(
    words: list[str],
) -> tuple[list[str], list[str], list[tuple[str, str]]] | None:
    """(words before git, words after push, git's global options) for a push.

    The first two are what a deletion check needs beyond the refspecs: an
    environment assignment before ``git`` (``GIT_DIR=...``) or a global option
    other than ``-C`` can point git at a repository other than the one the
    check reads, so a deletion carrying either is refused rather than checked
    against the wrong refs.
    """
    for start, word in enumerate(words):
        if word.rsplit("/", 1)[-1] != "git":
            continue
        globals_: list[tuple[str, str]] = []
        i = start + 1
        while i < len(words):
            token = words[i]
            if token in GIT_GLOBAL_WITH_ARG:
                value = words[i + 1] if i + 1 < len(words) else ""
                globals_.append((token, value))
                i += 2
            elif token.startswith("-"):
                globals_.append((token, ""))
                i += 1
            else:
                break
        if i < len(words) and words[i] == "push":
            return words[:start], words[i + 1 :], globals_
    return None


def git(cwd: str, *args: str) -> subprocess.CompletedProcess[str] | None:
    """Run one git command with a timeout; ``None`` if it could not run."""
    try:
        return subprocess.run(
            ["git", *args],
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=GIT_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def archive_tag(branch: str) -> str:
    """The tag name scripts/cleanup-branches.sh archives ``branch`` under."""
    return "archive/" + branch.replace("/", "-")


def preservation_failure(branch: str, cwd: str) -> str | None:
    """Why deleting ``branch`` on origin could lose work, or ``None`` if not.

    The branch's work is preserved when its tip, as this checkout last fetched
    it, is either already in origin/main or held by the archive tag. Every
    other outcome -- the ref missing, git failing or timing out, a tag at some
    other commit -- is a reason to refuse: this check exists to say "provably
    safe", and "could not tell" is not that.
    """
    tip = git(cwd, "rev-parse", "--verify", "-q", f"refs/remotes/origin/{branch}")
    if tip is None:
        return "git could not be run to find the branch's tip"
    tip_sha = tip.stdout.strip()
    if tip.returncode != 0 or not tip_sha:
        return (
            f"refs/remotes/origin/{branch} does not exist in {cwd}, so nothing "
            "proves its work is preserved (fetch first if it is new)"
        )

    merged = git(cwd, "merge-base", "--is-ancestor", tip_sha, "refs/remotes/origin/main")
    if merged is not None and merged.returncode == 0:
        return None

    tag = archive_tag(branch)
    tagged = git(cwd, "rev-parse", "--verify", "-q", f"refs/tags/{tag}^{{commit}}")
    if tagged is not None and tagged.returncode == 0 and tagged.stdout.strip() == tip_sha:
        return None

    if merged is None or merged.returncode not in (0, 1):
        return "git could not decide whether the branch is merged into origin/main"
    if tagged is not None and tagged.returncode == 0:
        return (
            f"it is not merged into origin/main, and the tag {tag} points at "
            f"{tagged.stdout.strip()[:12]}, not at the branch tip {tip_sha[:12]}"
        )
    return f"it is not merged into origin/main and no tag {tag} holds its tip"


def deletion_verdict(
    remote: str | None,
    branches: list[str],
    prefix: list[str],
    globals_: list[tuple[str, str]],
    cwd: str | None,
) -> tuple[str, str] | None:
    """Refuse a remote branch deletion unless every branch's work is preserved.

    On 2026-10-06 the owner asked for a fully clean branch list, which a guard
    refusing every deletion makes impossible except by routing around it. So a
    deletion is allowed exactly when it cannot lose work: never on the shared
    branches, and otherwise only for a branch whose tip is already in main or
    held by an archive tag. Anything the check cannot establish refuses.

    One limit, stated rather than hidden: the tip is read from this checkout's
    refs/remotes/origin/<branch>, as last fetched. A commit pushed to the
    branch after that fetch is not covered, and the deletion would remove it.
    Fetch immediately before deleting, as scripts/cleanup-branches.sh does.
    A local archive tag is likewise only proof once it has been pushed; the
    script pushes it before it deletes.
    """
    if not branches:
        return "a remote deletion naming no branch", DELETE_ADVICE
    if remote != DELETION_REMOTE:
        return (
            f"a remote branch deletion on {remote or 'an unnamed remote'}",
            f"Only deletions on {DELETION_REMOTE} can be checked against "
            f"refs/remotes/{DELETION_REMOTE}/main. Name {DELETION_REMOTE} explicitly.",
        )
    if any("=" in word for word in prefix):
        return (
            "a remote branch deletion with an environment override before git",
            "An override such as GIT_DIR can point git at a repository the "
            "check does not read. Run the deletion without it, using git -C.",
        )
    if any(option != "-C" for option, _ in globals_):
        return (
            "a remote branch deletion with a git global option other than -C",
            "Options such as --git-dir or -c can change which repository or "
            "remote the push reaches. Run the deletion with git -C only.",
        )

    directory = cwd if cwd is not None else os.getcwd()
    for option, value in globals_:
        directory = os.path.join(directory, value)

    for branch in branches:
        name = branch.removeprefix("refs/heads/")
        if name in PROTECTED_BRANCHES:
            return f"a remote branch deletion of the shared branch {name}", DELETE_ADVICE
        if (
            not name
            or name.startswith(("-", "refs/"))
            or name == "HEAD"
            or ":" in name
            or "*" in name
        ):
            return (
                f"a remote deletion of {branch}, which is not a plain branch name",
                "Only branches may be deleted this way, and only by name. Tags, "
                "especially archive/* tags, are the record of deleted work.",
            )
        reason = preservation_failure(name, directory)
        if reason is not None:
            return (
                f"a remote branch deletion of {name}: {reason}",
                PRESERVE_ADVICE.format(tag=archive_tag(name).removeprefix("archive/")),
            )
    return None


def refuse_push(
    args: list[str],
    prefix: list[str] | None = None,
    globals_: list[tuple[str, str]] | None = None,
    cwd: str | None = None,
) -> tuple[str, str] | None:
    """Return (what was refused, what to do instead) for a dangerous push."""
    positional: list[str] = []
    i = 0
    options_done = False
    deleting = False
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
                if PUSH_LONG_DELETE.startswith(name):
                    deleting = True
                    continue
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
            if flag == PUSH_SHORT_DELETE:
                deleting = True
                continue
            if flag in PUSH_SHORT_REFUSED:
                refused, deletes = PUSH_SHORT_REFUSED[flag]
                return refused, DELETE_ADVICE if deletes else FORCE_ADVICE
            if flag in PUSH_SHORT_WITH_ARG:
                if position == len(token) - 2:
                    i += 1
                break

    # The first positional word is the repository; the rest are refspecs.
    # Under --delete every refspec is a branch to delete.
    deletions: list[str] = []
    for refspec in positional[1:]:
        if refspec == ":":
            return (
                "a push of every matching branch, the shared branches included",
                "Name the one branch you mean to push.",
            )
        if refspec.startswith("+"):
            return "a force push (a '+' refspec forces that ref)", FORCE_ADVICE
        if deleting:
            deletions.append(refspec)
        elif refspec.startswith(":"):
            deletions.append(refspec[1:])
    if deleting or deletions:
        remote = positional[0] if positional else None
        return deletion_verdict(remote, deletions, prefix or [], globals_ or [], cwd)
    return None


def push_verdict(
    text: str, depth: int = 0, cwd: str | None = None
) -> tuple[str, str] | None:
    """The first dangerous push in ``text``, looking inside quoted scripts.

    ``bash -c "git push ..."`` hands the whole push to the shell as one quoted
    word, which the tokeniser rightly keeps whole -- so a word that itself
    contains a push is parsed again as a command line, to a bounded depth.
    """
    commands = segments(text)
    # A "cd" anywhere in the line means git may run somewhere other than the
    # directory the deletion check reads, so a deletion is refused outright.
    changes_directory = any(
        words and words[0] in ("cd", "pushd", "popd") for words in commands
    )
    for words in commands:
        invocation = push_invocation(words)
        if invocation is not None:
            prefix, args, globals_ = invocation
            verdict = refuse_push(args, prefix, globals_, cwd)
            if verdict is not None:
                return verdict
            if changes_directory and is_deletion(args):
                return (
                    "a remote branch deletion after a change of directory",
                    "The check reads the directory the command starts in. "
                    "Use git -C <dir> instead of cd.",
                )
        if depth < 3:
            for word in words:
                if "push" in word and any(c.isspace() for c in word):
                    verdict = push_verdict(word, depth + 1, cwd)
                    if verdict is not None:
                        return verdict
    return None


def is_deletion(args: list[str]) -> bool:
    """Whether a push's arguments delete a remote ref, by any spelling."""
    for token in args:
        if token == "--":
            break
        if token.startswith("--"):
            name = token[2:].partition("=")[0]
            if len(name) >= 2 and PUSH_LONG_DELETE.startswith(name):
                return True
        elif token.startswith("-") and PUSH_SHORT_DELETE in token[1:]:
            return True
    return any(word.startswith(":") for word in args)


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
