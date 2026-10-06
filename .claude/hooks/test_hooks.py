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
import tempfile

HERE = pathlib.Path(__file__).parent
GUARD = [sys.executable, str(HERE / "guard-dangerous-command.py")]
FORMAT = [sys.executable, str(HERE / "format-rust-after-edit.py")]

# Assembled so this file does not trip the guard when a shell passes it around.
TF = "terraform "
PUSH = "git " + "push "
FORCE = "--" + "force"
DELETE = "--" + "delete"
SHORT_DELETE = "-" + "d"

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

# The protected-branch bypasses a review found on 2026-10-06. Every one of
# them reached main or the integration branch past PROTECTED_PUSH, the regex
# that reads raw text: a quote beside the name, git's -C, a shell re-reading a
# quoted script, a heredoc fed to a shell, a destination git takes from the
# checked-out branch, and configuration that turns a push into a force.
#
# (command, repository the command runs in, expected, property). The
# repository matters because "HEAD", a bare push and --all go to whatever
# branch is checked out: "main" is a repository on main, "lane" one on lane/x
# (with a local main beside it), "tracking" one on lane/y whose push goes to
# origin/main, and "nogit" a directory that is no repository at all. "{main}"
# in a command is replaced by the main repository's path.
MAIN = "ma" + "in"
SUB = "pu" + "sh "  # the subcommand alone, after git's own options
INTEGRATION = "ccr-0c1bacf8-" + "kla0dd"
PROTECTED_CASES: list[tuple[str, str, int, str]] = [
    (PUSH + 'origin "' + MAIN + '"', "lane", 2, "a double-quoted main is refused"),
    (PUSH + "origin 'HEAD:" + MAIN + "'", "lane", 2, "a quoted HEAD:main is refused"),
    (PUSH + "origin ma''in", "lane", 2, "main split by empty quotes is refused"),
    (
        PUSH + 'origin "HEAD:refs/heads/' + INTEGRATION + '"',
        "lane",
        2,
        "a quoted, qualified push to the integration branch is refused",
    ),
    ("git -C /tmp " + SUB + "origin " + MAIN, "lane", 2, "main behind -C is refused"),
    ("git -C {main} " + SUB + "origin HEAD", "lane", 2, "-C into a repo on main is followed"),
    ("cd {main} && " + PUSH, "lane", 2, "a cd into a repo on main is followed"),
    ('bash -c "' + PUSH + "origin " + MAIN + '"', "lane", 2, "main inside bash -c is refused"),
    ("sh -c '" + PUSH + 'origin "' + MAIN + "\"'", "lane", 2, "quoted main inside sh -c is refused"),
    ('/bin/bash -c "git pu\'\'sh origin ' + MAIN + '"', "lane", 2, "pu''sh inside bash -c is refused"),
    ("bash <<EOF\n" + PUSH + "origin " + MAIN + "\nEOF", "lane", 2, "a heredoc fed to bash is refused"),
    ("/bin/sh -s <<'EOF'\n" + PUSH + "origin HEAD:" + INTEGRATION + "\nEOF", "lane", 2, "a quoted heredoc fed to /bin/sh is refused"),
    ("cat <<EOF | zsh\n" + PUSH + "origin " + MAIN + "\nEOF", "lane", 2, "a heredoc piped to zsh is refused"),
    ("dash <<EOF\n" + PUSH + "\nEOF", "main", 2, "a bare push in a heredoc to dash on main is refused"),
    (PUSH + "origin HEAD", "main", 2, "pushing HEAD while on main is refused"),
    (PUSH.strip(), "main", 2, "a bare push while on main is refused"),
    (PUSH + "origin", "main", 2, "a push naming only the remote while on main is refused"),
    (PUSH + "--follow-tags", "main", 2, "a bare push with --follow-tags while on main is refused"),
    (PUSH + "--all origin", "main", 2, "--all while on main is refused"),
    (PUSH + "--all origin", "lane", 2, "--all with a local main beside the lane is refused"),
    (PUSH + "origin @", "main", 2, "pushing @ while on main is refused"),
    (PUSH, "nogit", 2, "a bare push where the branch cannot be read is refused"),
    (PUSH + "origin HEAD", "nogit", 2, "HEAD where the branch cannot be read is refused"),
    (PUSH, "tracking", 2, "a bare push whose @{push} is origin/main is refused"),
    (PUSH + "origin 'refs/heads/*:refs/heads/*'", "lane", 2, "a wildcard destination is refused"),
    (
        "git -c remote.origin.push=+refs/heads/*:refs/heads/" + MAIN + " " + SUB,
        "lane",
        2,
        "a push refspec set by -c is refused",
    ),
    ("git -c push.default=matching " + SUB, "lane", 2, "a push.* default set by -c is refused"),
    ("git -c alias.p='push " + FORCE + "' p", "lane", 2, "a -c alias to a forced push is refused"),
    ("git -c alias.st=status st", "lane", 2, "any -c alias is refused outright"),
    # Allow cases: the same machinery must still let a lane publish itself.
    (PUSH + "-u origin lane/L001-b-x", "main", 0, "an explicit lane push is allowed even on main"),
    (PUSH + "origin feature-f", "lane", 0, "a branch ending in -f is still allowed"),
    (PUSH + "-u origin lane/L001-b-main-x", "lane", 0, "a lane containing main is allowed"),
    (PUSH + "origin HEAD", "lane", 0, "pushing HEAD while on lane/x is allowed"),
    (PUSH.strip(), "lane", 0, "a bare push while on lane/x is allowed"),
    (PUSH + "-u origin HEAD:lane/x", "lane", 0, "HEAD to an explicit lane is allowed"),
    (PUSH + "origin lane/x", "nogit", 0, "an explicit lane needs no branch lookup"),
    (PUSH + "--tags origin", "main", 0, "a tags-only push touches no branch"),
    ("git -c user.name=x commit -m y", "lane", 0, "-c on a command other than push is allowed"),
    ("cat > d.md <<EOF\n" + PUSH + "origin " + MAIN + "\nEOF", "lane", 0, "a heredoc written to a file is allowed"),
    ("git commit -m 'teach the guard bash and ma\"\"in'", "lane", 0, "a commit message is not re-read as a script"),
]


# Remote branch deletion, decided against a real repository. Each case is
# (command, expected exit, property). The fixture below holds, as
# refs/remotes/origin/*: main; the integration branch, at a commit already in
# main so that only its protection can refuse it; "merged", in main;
# "unmerged", not in main and untagged; "feature/archived", not in main but
# held by archive/feature-archived at its exact tip; and "wrong-tip", not in
# main, with archive/wrong-tip pointing at a different commit -- the case
# that proves the tag is compared with the tip rather than merely found.
DELETION_CASES: list[tuple[str, int, str]] = [
    (PUSH + "origin " + DELETE + " merged", 0, "deleting a merged branch is allowed"),
    (PUSH + "origin " + SHORT_DELETE + " merged", 0, "-d of a merged branch is allowed"),
    (PUSH + "origin :merged", 0, "the ':' deletion of a merged branch is allowed"),
    (
        PUSH + "origin " + DELETE + " refs/heads/merged",
        0,
        "a fully qualified merged branch is allowed",
    ),
    (PUSH + "origin " + DELETE + " unmerged", 2, "deleting an unmerged branch is refused"),
    (PUSH + "origin :unmerged", 2, "the ':' deletion of an unmerged branch is refused"),
    (
        PUSH + "origin " + DELETE + " feature/archived",
        0,
        "an unmerged branch archived at its tip may be deleted",
    ),
    (
        PUSH + "origin " + DELETE + " wrong-tip",
        2,
        "an unmerged branch whose archive tag is at another commit is refused",
    ),
    (PUSH + "origin " + DELETE + " main", 2, "deleting main is refused"),
    (PUSH + "origin :main", 2, "the ':main' deletion is refused"),
    (
        PUSH + "origin " + DELETE + " ccr-0c1bacf8-kla0dd",
        2,
        "deleting the integration branch is refused although it is merged",
    ),
    (
        PUSH + "origin :refs/heads/ccr-0c1bacf8-kla0dd",
        2,
        "a qualified ':' deletion of the integration branch is refused",
    ),
    (PUSH + "origin " + DELETE + " no-such-branch", 2, "an unknown branch is refused"),
    (
        PUSH + "origin " + DELETE + " merged unmerged",
        2,
        "one unpreserved branch refuses the whole deletion",
    ),
    (
        PUSH + "origin :refs/tags/archive/feature-archived",
        2,
        "deleting an archive tag is refused",
    ),
    (PUSH + "upstream " + DELETE + " merged", 2, "a remote other than origin is refused"),
    (PUSH + DELETE, 2, "a deletion naming nothing is refused"),
    (PUSH + "origin " + DELETE + " merged " + FORCE, 2, "a forced deletion is refused"),
    (PUSH + "origin " + DELETE + " --mirror", 2, "a mirror deletion is refused"),
    (PUSH + "origin --prune merged", 2, "a pruning push is still refused"),
    (
        "GIT_DIR=/elsewhere " + PUSH + "origin " + DELETE + " merged",
        2,
        "a deletion with an environment override is refused",
    ),
    (
        "git --git-dir=/elsewhere " + PUSH[4:] + "origin " + DELETE + " merged",
        2,
        "a deletion with --git-dir is refused",
    ),
    (
        "cd /tmp && " + PUSH + "origin " + DELETE + " merged",
        2,
        "a deletion after a cd is refused",
    ),
    (
        'bash -c "' + PUSH + "origin " + DELETE + ' unmerged"',
        2,
        "an unmerged deletion inside bash -c is refused",
    ),
]


def git(cwd: pathlib.Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=True
    ).stdout.strip()


def build_fixture(root: pathlib.Path) -> pathlib.Path:
    """A repository whose origin tracking refs exercise every deletion arm."""
    repo = root / "repo"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "work")
    git(repo, "config", "user.email", "fixture@example.invalid")
    git(repo, "config", "user.name", "fixture")
    git(repo, "config", "commit.gpgsign", "false")
    git(repo, "config", "tag.gpgsign", "false")

    def commit(message: str) -> str:
        git(repo, "commit", "-q", "--allow-empty", "-m", message)
        return git(repo, "rev-parse", "HEAD")

    root_commit = commit("root")
    merged = commit("merged work")
    main = commit("main")
    git(repo, "checkout", "-q", "-b", "side", root_commit)
    unmerged = commit("unmerged work")
    archived = commit("archived work")
    wrong_tip = commit("wrong-tip work")

    for branch, sha in {
        "main": main,
        "ccr-0c1bacf8-kla0dd": merged,
        "merged": merged,
        "unmerged": unmerged,
        "feature/archived": archived,
        "wrong-tip": wrong_tip,
    }.items():
        git(repo, "update-ref", f"refs/remotes/origin/{branch}", sha)
    git(repo, "tag", "-a", "-m", "archived", "archive/feature-archived", archived)
    git(repo, "tag", "-a", "-m", "stale", "archive/wrong-tip", archived)

    # The premises, asserted: a fixture that silently merged everything
    # would make every allow case pass and prove nothing.
    def ancestor(sha: str) -> bool:
        return subprocess.run(
            ["git", "merge-base", "--is-ancestor", sha, main], cwd=repo
        ).returncode == 0

    assert ancestor(merged), "fixture: merged is not in main"
    assert not ancestor(unmerged), "fixture: unmerged is in main"
    assert not ancestor(archived), "fixture: archived is in main"
    assert not ancestor(wrong_tip), "fixture: wrong-tip is in main"
    assert wrong_tip != archived, "fixture: wrong-tip's tag is at its own tip"
    return repo


def load_guard():
    spec = importlib.util.spec_from_file_location(
        "guard", HERE / "guard-dangerous-command.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(argv: list[str], payload: object, cwd: str | pathlib.Path | None = None) -> int:
    return subprocess.run(
        argv, input=json.dumps(payload), capture_output=True, text=True, cwd=cwd
    ).returncode


def make_repositories(root: pathlib.Path) -> dict[str, str]:
    """Throwaway repositories checked out where each case needs them."""

    def git(cwd: pathlib.Path, *args: str) -> None:
        subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid",
             *args],
            cwd=cwd, check=True, capture_output=True,
        )

    repos: dict[str, str] = {}
    for name, branch in (("main", None), ("lane", "lane/x"), ("tracking", "lane/y")):
        path = root / name
        path.mkdir()
        git(path, "init", "-q", "-b", MAIN)
        git(path, "commit", "-q", "--allow-empty", "-m", "seed")
        if branch:
            git(path, "checkout", "-q", "-b", branch)
        repos[name] = str(path)
    tracking = root / "tracking"
    git(tracking, "remote", "add", "origin", str(root / "nowhere.git"))
    git(tracking, "update-ref", "refs/remotes/origin/" + MAIN, "HEAD")
    git(tracking, "branch", "-q", "--set-upstream-to=origin/" + MAIN)
    git(tracking, "config", "push.default", "upstream")
    (root / "nogit").mkdir()
    repos["nogit"] = str(root / "nogit")
    return repos


def refusal(command: str, cwd: pathlib.Path) -> str:
    return subprocess.run(
        GUARD,
        input=json.dumps({"tool_input": {"command": command}}),
        capture_output=True,
        text=True,
        cwd=cwd,
    ).stderr


def main() -> int:
    with tempfile.TemporaryDirectory() as scratch:
        return run_all(build_fixture(pathlib.Path(scratch)), pathlib.Path(scratch))


def run_all(repo: pathlib.Path, scratch: pathlib.Path) -> int:
    failures = 0
    repos_dir = tempfile.TemporaryDirectory()
    repos = make_repositories(pathlib.Path(repos_dir.name))

    # Run from a repository on a lane so that cases pushing HEAD do not depend
    # on which branch the person running these tests has checked out.
    for command, expected, name in GUARD_CASES + PUSH_CASES:
        got = run(GUARD, {"tool_input": {"command": command}}, cwd=repos["lane"])
        if got != expected:
            failures += 1
            print(f"FAIL  {name}: expected exit {expected}, got {got}")
        else:
            print(f"ok    {name}")

    # Every deletion case runs inside the fixture, so its answer depends on
    # refs this file built rather than on whatever the caller's checkout
    # happens to have fetched.
    for command, expected, name in DELETION_CASES:
        got = run(GUARD, {"tool_input": {"command": command}}, cwd=repo)
        if got != expected:
            failures += 1
            print(f"FAIL  {name}: expected exit {expected}, got {got}")
        else:
            print(f"ok    {name}")

    guard = load_guard()
    for command, expected, name in PUSH_CASES:
        verdict = guard.push_verdict(guard.strip_heredocs(command), cwd=repos["lane"])
        got = 0 if verdict is None else 2
        if got != expected:
            failures += 1
            print(f"FAIL  push parser alone, {name}: expected {expected}, got {got}")
        else:
            print(f"ok    push parser alone: {name}")
    for command, expected, name in DELETION_CASES:
        verdict = guard.push_verdict(guard.strip_heredocs(command), cwd=str(repo))
        got = 0 if verdict is None else 2
        if got != expected:
            failures += 1
            print(f"FAIL  push parser alone, {name}: expected {expected}, got {got}")
        else:
            print(f"ok    push parser alone: {name}")

    # Each protected case runs end to end with the hook's working directory
    # in the named repository, and again through push_verdict alone with that
    # repository passed as cwd: the regex layer still catches a plain
    # "origin main", so only the parser-alone run proves the new check fires.
    for template, where, expected, name in PROTECTED_CASES:
        command = template.replace("{main}", repos["main"])
        got = run(GUARD, {"tool_input": {"command": command}}, cwd=repos[where])
        if got != expected:
            failures += 1
            print(f"FAIL  {name} [{where}]: expected exit {expected}, got {got}")
        else:
            print(f"ok    {name} [{where}]")
        verdict = guard.push_verdict(guard.strip_heredocs(command), cwd=repos[where])
        got = 0 if verdict is None else 2
        if got != expected:
            failures += 1
            print(f"FAIL  push parser alone, {name} [{where}]: expected {expected}, got {got}")
        else:
            print(f"ok    push parser alone: {name} [{where}]")

    # The payload's cwd, not the hook process's, decides where HEAD is read.
    got = run(
        GUARD,
        {"tool_input": {"command": PUSH + "origin HEAD"}, "cwd": repos["main"]},
        cwd=repos["lane"],
    )
    if got != 2:
        failures += 1
        print(f"FAIL  the payload cwd on main was ignored (exit {got})")
    else:
        print("ok    the payload's cwd decides which branch HEAD is")

    # git -C decides where the check reads, wherever the hook itself runs.
    outside = scratch / "not-a-repo"
    outside.mkdir()
    for command, expected, name in [
        (
            "git -C " + str(repo) + " " + PUSH[4:] + "origin " + DELETE + " merged",
            0,
            "a merged deletion behind -C is checked in that repository",
        ),
        (
            "git -C " + str(repo) + " " + PUSH[4:] + "origin " + DELETE + " unmerged",
            2,
            "an unmerged deletion behind -C is refused",
        ),
        (
            PUSH + "origin " + DELETE + " merged",
            2,
            "a deletion where git cannot run is refused (fail closed)",
        ),
    ]:
        got = run(GUARD, {"tool_input": {"command": command}}, cwd=outside)
        if got != expected:
            failures += 1
            print(f"FAIL  {name}: expected exit {expected}, got {got}")
        else:
            print(f"ok    {name}")

    # The refusal must say what to do instead, naming the exact tag.
    message = refusal(PUSH + "origin " + DELETE + " feature/unmerged-x", repo)
    if "archive/feature-unmerged-x" not in message or "Merge it" not in message:
        failures += 1
        print(f"FAIL  the refusal names no remedy: {message!r}")
    else:
        print("ok    the refusal names the merge-or-archive remedy and the tag")

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
