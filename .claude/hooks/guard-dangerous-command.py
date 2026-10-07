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
        ("terraform apply", "prod"),
        "a Terraform apply against production",
        "Production deployments must go through .github/workflows/infra.yml, "
        "which is refused by the Terraform gate. Use 'terraform plan' to review "
        "changes first, then request production approval.",
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

    # None means a cd made the directory unknowable (push_verdict); the
    # refs this check reads would then belong to some other repository.
    if cwd is None:
        return (
            "a remote branch deletion where the working directory is unknown",
            "Run the deletion with git -C <dir> and no cd before it.",
        )
    directory = cwd
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


# --- Protected destinations, resolved the way git resolves them ------------
#
# PROTECTED_PUSH above reads raw text, and on 2026-10-06 a security review
# listed the pushes to main it let through: a quote beside the name
# ("main", 'HEAD:main', ma''in), git's own -C, a push inside bash -c, a push
# in a heredoc fed to a shell, and every push whose destination is not written
# at all -- "HEAD", a bare "git push", "--all" -- which go to whatever branch
# is checked out. So the destination is now resolved from the shlex tokens the
# force/delete parser already reads, and from the repository the push would
# actually run in. The regex is kept as a second, independent layer.

PROTECTED_ADVICE = (
    "Push your own branch and open a pull request against it; the "
    "orchestrator reviews and merges."
)
# `git -c` reconfigures the command it precedes. On a push it can install a
# forcing refspec (remote.<name>.push=+...) or a push.* default the parser
# never sees; as alias.* it can turn any word into "push --force".
CONFIG_REFUSED = (
    "a git -c configuration on a push, which can force or redirect it "
    "where the arguments do not show it",
    "Run the push without -c; set configuration you need with an ordinary "
    "reviewed command first.",
)
ALIAS_REFUSED = (
    "a git -c alias, which can make any word run a push the guard cannot read",
    "Run the git subcommand by its own name.",
)
SHELLS = {"bash", "sh", "zsh", "dash", "ksh"}


def git_invocation(
    words: list[str],
) -> tuple[dict[str, str], list[str], list[str], str | None, list[str]] | None:
    """(env prefix, global options, -c keys, subcommand, its arguments)."""
    for start, word in enumerate(words):
        if word.rsplit("/", 1)[-1] != "git":
            continue
        env = {}
        for prefix in words[:start]:
            name, eq, value = prefix.partition("=")
            if eq and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
                env[name] = value
        globals_: list[str] = []
        config_keys: list[str] = []
        i = start + 1
        while i < len(words):
            token = words[i]
            if token in ("-c", "--config-env"):
                if i + 1 < len(words):
                    config_keys.append(words[i + 1].partition("=")[0].lower())
                i += 2
            elif token.startswith("--config-env="):
                config_keys.append(token.split("=", 2)[1].lower())
                i += 1
            elif token in GIT_GLOBAL_WITH_ARG:
                globals_ += words[i : i + 2]
                i += 2
            elif token.startswith("-"):
                globals_.append(token)
                i += 1
            else:
                break
        sub = words[i] if i < len(words) else None
        return env, globals_, config_keys, sub, words[i + 1 :]
    return None


def push_destinations(args: list[str]) -> tuple[list[str], str | None]:
    """Explicit destinations, and what an absent refspec defaults to.

    The second value is None when explicit refspecs decide everything,
    "current" when git falls back to the checked-out branch, and "all" for
    --all/--branches. Option skipping mirrors refuse_push, which has already
    refused every force and delete spelling by the time this runs.
    """
    positional: list[str] = []
    all_branches = tags = repo_option = False
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
            if len(name) >= 2 and ("all".startswith(name) or "branches".startswith(name)):
                all_branches = True
            elif len(name) >= 2 and "tags".startswith(name):
                tags = True
            elif len(name) >= 2 and any(o.startswith(name) for o in PUSH_LONG_WITH_ARG):
                repo_option = repo_option or "repo".startswith(name)
                if not has_value:
                    i += 1
            continue
        for position, flag in enumerate(token[1:]):
            if flag in PUSH_SHORT_WITH_ARG:
                if position == len(token) - 2:
                    i += 1
                break

    # With --repo, git lets a positional word override it as the repository,
    # so the first word may be either; read it as a refspec too.
    refspecs = positional if repo_option else positional[1:]
    destinations: list[str] = []
    for refspec in refspecs:
        src, colon, dst = refspec.lstrip("+").partition(":")
        destinations.append(dst if colon and dst else src)
    if all_branches:
        return destinations, "all"
    if not refspecs and not tags:
        return destinations, "current"
    return destinations, None


def branch_name(ref: str) -> str:
    for prefix in ("refs/heads/", "heads/"):
        if ref.startswith(prefix):
            return ref[len(prefix) :]
    return ref


def git_lookup(cwd: str | None, env: dict[str, str], globals_: list[str],
               args: list[str]) -> str | None:
    """Run a read-only git query where the push would run; None on failure."""
    if cwd is None or any("$" in g or "`" in g for g in globals_):
        return None
    try:
        result = subprocess.run(
            ["git", *globals_, *args],
            cwd=cwd,
            env={**os.environ, **env},
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def refuse_protected(words: list[str], cwd: str | None) -> tuple[str, str] | None:
    """Refuse a git command that would update main or the integration branch."""
    parsed = git_invocation(words)
    if parsed is None:
        return None
    env, globals_, config_keys, sub, args = parsed
    if any(key.startswith("alias.") for key in config_keys):
        return ALIAS_REFUSED
    if sub != "push":
        return None
    if config_keys:
        return CONFIG_REFUSED

    destinations, default = push_destinations(args)
    if default is not None:
        current = git_lookup(cwd, env, globals_, ["rev-parse", "--abbrev-ref", "HEAD"])
        if current is None:
            return (
                "a push of the current branch where the current branch could "
                "not be determined",
                "Name the destination explicitly: git push origin <your-branch>.",
            )
        destinations.append(current)
        if default == "all":
            local = git_lookup(
                cwd, env, globals_,
                ["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            )
            if local is None:
                return (
                    "a push of every branch where the local branches could "
                    "not be listed",
                    "Name the one branch you mean to push.",
                )
            destinations += local.splitlines()
        else:
            # push.default=upstream or a remote.<name>.push refspec can send
            # the current branch somewhere other than its own name. Best
            # effort: a branch with no upstream yet has no @{push}.
            upstream = git_lookup(
                cwd, env, globals_, ["rev-parse", "--symbolic-full-name", "@{push}"]
            )
            if upstream and upstream.startswith("refs/remotes/"):
                remotes = git_lookup(cwd, env, globals_, ["remote"]) or ""
                for remote in remotes.splitlines():
                    prefix = f"refs/remotes/{remote}/"
                    if upstream.startswith(prefix):
                        destinations.append(upstream[len(prefix) :])

    for destination in destinations:
        if destination in ("HEAD", "@"):
            current = git_lookup(cwd, env, globals_, ["rev-parse", "--abbrev-ref", "HEAD"])
            if current is None:
                return (
                    "a push of HEAD where the current branch could not be "
                    "determined",
                    "Name the destination explicitly: git push origin <your-branch>.",
                )
            destination = current
        name = branch_name(destination)
        if "*" in name:
            return (
                f"a wildcard push ({destination}) that can match a shared branch",
                "Name the one branch you mean to push.",
            )
        if name in PROTECTED_BRANCHES:
            return f"a direct push to the shared branch {name}", PROTECTED_ADVICE
    return None


def next_cwd(words: list[str], cwd: str | None) -> str | None:
    """The directory after a ``cd``/``pushd``; None once it cannot be known."""
    if cwd is None or len(words) != 2 or words[1] == "-":
        return None
    target = os.path.expanduser(words[1])
    if "$" in target or "`" in target or target.startswith("~"):
        return None
    return os.path.normpath(os.path.join(cwd, target))


def push_verdict(
    text: str, depth: int = 0, cwd: str | None = "."
) -> tuple[str, str] | None:
    """The first dangerous push in ``text``, looking inside quoted scripts.

    ``bash -c "git push ..."`` hands the whole push to the shell as one quoted
    word, which the tokeniser rightly keeps whole -- so a word that itself
    contains a push is parsed again as a command line, to a bounded depth.

    ``cwd`` is where the command runs; None means a ``cd`` made it unknowable,
    and then a push of the current branch is refused rather than guessed at.
    """
    if cwd == ".":
        cwd = os.getcwd()
    commands = segments(text)
    # A "cd" anywhere in the line means git may run somewhere other than the
    # directory the deletion check reads, so a deletion is refused outright.
    changes_directory = any(
        words and words[0] in ("cd", "pushd", "popd") for words in commands
    )
    for words in commands:
        if words and words[0] in ("cd", "pushd"):
            cwd = next_cwd(words, cwd)
            continue
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
        verdict = refuse_protected(words, cwd)
        if verdict is not None:
            return verdict
        if depth < 3:
            # A shell, eval or watch runs a quoted word as a script, so every
            # multi-word argument there is re-read. Elsewhere only a word that
            # mentions a push is: a spelling the shell would join back
            # together (pu''sh) only arrives through a shell.
            runs_script = any(
                w.rsplit("/", 1)[-1] in SHELLS | {"eval", "watch"} for w in words
            )
            for word in words:
                multiword = any(c.isspace() or c in ";&|" for c in word)
                if multiword and (runs_script or "push" in word):
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

    Except when the heredoc is fed to a shell: ``bash <<EOF`` runs its body,
    and stripping it hid a push to main from every check (2026-10-06). A
    heredoc on a line naming a shell -- ``bash``, ``/bin/sh -s``, ``cat <<EOF
    | sh`` -- keeps its body as commands to inspect.
    """
    while True:
        match = HEREDOC.search(text)
        if match is None:
            return text
        line_start = text.rfind("\n", 0, match.start()) + 1
        rest = text[match.end() :]
        line_end = rest.find("\n")
        line = text[line_start : match.start()] + (rest if line_end < 0 else rest[:line_end])
        runs_body = any(
            word.rsplit("/", 1)[-1] in SHELLS for words in segments(line) for word in words
        )
        terminator = re.search(
            r"^\s*" + re.escape(match.group(2)) + r"\s*$", rest, re.M
        )
        if terminator is None:
            # An unterminated heredoc: everything after the marker is body.
            return text if runs_body else text[: match.end()]
        if runs_body:
            # Drop the marker and the terminator; keep the body as commands.
            text = text[: match.start()] + rest[: terminator.start()] + rest[terminator.end() :]
        else:
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

    # The payload names the directory the command will run in; the hook's own
    # working directory is only the fallback.
    cwd = payload.get("cwd")
    verdict = push_verdict(inspected, cwd=cwd if isinstance(cwd, str) and cwd else ".")
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
