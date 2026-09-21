#!/usr/bin/env python3
"""Refuse a YAML file under infrastructure/ that no parser can read.

Every gate this repository ran before 2026-09-21 read these manifests as
*text*. `gitops.rs` counts the files whose serialised bytes contain a
principal; `console_route.rs` scans for a `member:` line; `terraform fmt`
never opens them at all. Text matching cannot tell a well-formed document
from a broken one, so a malformed manifest passed `make check`, passed CI,
and reached `main`.

What found it instead was Argo CD, ten minutes into an `infra.yml` `apps`
dispatch, as

  ComparisonError: Failed to load target state: ... `kustomize build
  .../infrastructure/gitops/envs/dev` failed exit status 1: Error:
  accumulating resources from 'invokers.yaml': MalformedYAMLError:
  yaml: line 14: mapping values are not allowed in this context

— a colon inside an unquoted annotation sentence, in a comment explaining
why a grant exists. The Application reported `sync=Unknown`, every object
reported an empty health, and the diagnostic's per-object results were
frozen at a *previous* attempt's Artifact Registry 403, so the run read as
a permission problem that had in fact been fixed hours earlier. That is the
cost being prevented here: not the typo, but the eight-minute wait and the
wrong cause at the end of it.

The check is deliberately the weakest one that would have fired. It proves
each file parses; it does not check schema, field names, or whether
Kubernetes would accept the object, because a check that tried to do that
would need a cluster and would therefore not run here. `kustomize build`
would be stronger still — it also catches a patch that targets nothing —
and needs a binary this script must not assume.
"""

import pathlib
import sys

try:
    import yaml
except ImportError:  # pragma: no cover - the refusal is the behaviour
    # Fail closed. A gate that skips itself when a library is absent reports
    # success on a file nobody read, which is the failure mode this whole
    # script exists to remove.
    print(
        "check-manifests: PyYAML is not installed, so nothing was parsed.\n"
        "Install it (pip install pyyaml) and run again. This check refuses\n"
        "rather than passing, because a skipped parse and a clean parse must\n"
        "never print the same thing.",
        file=sys.stderr,
    )
    raise SystemExit(1)

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCOPE = ROOT / "infrastructure"


def main() -> int:
    files = sorted(
        path
        for pattern in ("**/*.yaml", "**/*.yml")
        for path in SCOPE.glob(pattern)
        if path.is_file()
    )
    if not files:
        # An empty scan and a clean scan read identically in a log, and only
        # one of them means anything.
        print(
            f"check-manifests: no YAML found under {SCOPE.relative_to(ROOT)}; "
            "the scope is wrong, not the tree.",
            file=sys.stderr,
        )
        return 1

    broken = []
    for path in files:
        try:
            # safe_load_all, not safe_load: every manifest here is a multi-
            # document file, and safe_load would refuse the `---` that
            # separates them while saying nothing about the documents after
            # the first.
            # The file object rather than its text, so PyYAML's own error
            # mark names the path instead of "<unicode string>" — the
            # message has to be usable when it is the only thing a reader
            # of a CI log has.
            with path.open(encoding="utf-8") as handle:
                list(yaml.safe_load_all(handle))
        except yaml.YAMLError as error:
            broken.append((path, error))

    for path, error in broken:
        print(f"{path.relative_to(ROOT)}: {error}", file=sys.stderr)

    if broken:
        print(
            f"\ncheck-manifests: {len(broken)} of {len(files)} file(s) under "
            f"{SCOPE.relative_to(ROOT)} do not parse as YAML.\n"
            "Argo CD renders these with kustomize and refuses the whole\n"
            "Application when any one of them is malformed, so this is a\n"
            "deployment that fails ten minutes in, naming a sync timeout\n"
            "rather than the file above.",
            file=sys.stderr,
        )
        return 1

    print(f"manifest parse: {len(files)} YAML file(s) parse, all permitted")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
