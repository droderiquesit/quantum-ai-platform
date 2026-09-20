#!/usr/bin/env python3
"""The resources Google keeps after Terraform has stopped managing them.

Two commands read one parse of the module sources, and that sharing is the
whole point of this file. `infra.yml`'s `teardown` takes those resources *out
of state* and leaves them standing in the project; `infra.yml`'s `up` has to
put them *back into state* before it applies, or it plans to create objects
that are already there. The two steps have to agree on one list, and they did
not: the teardown derived its list from the sources — after run 41 aborted on
the evidence bucket, which a hand-typed pattern for keys had missed — and the
`up` side had no list at all. Run 44 planned `184 to add, 0 to change, 0 to
destroy` against a project still holding nine crypto keys, a key ring and five
buckets, every one of which answers a create with "already exists". An apply of
that plan gets partway through and leaves a half-built environment billing.

    addresses   the set the teardown removes from state, one `type.name` per
                line — the parse, unchanged.

    imports     read a plan as JSON and say which of its *creates* would
                collide with something the teardown left behind, with the
                identifier `terraform import` wants and a probe the caller uses
                to ask Google whether the object is actually there.

This file resolves nothing against Google and imports nothing. It reads a plan
and prints lines. Every effect — the probe, the import — is the caller's, in
`infra.yml`, where it can be read.
"""

import argparse
import glob
import json
import os
import re
import sys

# A component that will reach a shell as a word. Everything printed below is a
# project id, a location, a key ring name, a key name or a bucket name, and
# Google's own naming rules draw every one of those from this set. A value that
# is not in it is refused rather than quoted: the caller splits these fields,
# and a value carrying a shell metacharacter is not a malformed name, it is
# somebody else's command. The same argument `infra.yml`'s identity step makes
# about a project id read out of the tfvars.
SAFE = re.compile(r"^[a-zA-Z0-9][a-zA-Z0-9._-]*$")

# `projects/P/locations/L/keyRings/R`, which is what a crypto key's `key_ring`
# holds once the ring itself is in state.
KEY_RING_PATH = re.compile(r"^projects/([^/]+)/locations/([^/]+)/keyRings/([^/]+)$")

# Types whose *name* survives their own destruction, whatever the sources say
# about `prevent_destroy`.
#
# This is not a hand-typed list of resources — the addresses still come from the
# parse below, and this names two types. It is a statement about two Google
# APIs that the sources cannot carry. A key ring has no delete method at all:
# `terraform destroy` drops it from state with a warning and the name stays
# claimed forever. A crypto key can only be *scheduled* for destruction, and its
# name stays claimed then too. Both therefore collide with a later create even
# though the destroy reported success — and `google_kms_key_ring.platform` in
# `modules/secrets` declares no lifecycle block, so it is invisible to the
# parse. It is the one resource in this category the teardown's own list has
# never named, and the one an `up` would hit first, because every crypto key
# hangs off it.
NAME_SURVIVES_DESTRUCTION = (
    "google_kms_key_ring",
    "google_kms_crypto_key",
    # Identity Platform's configuration is one per project and cannot be
    # deleted at all: `modules/identity` keeps `identitytoolkit.googleapis.com`
    # on through a destroy (`disable_on_destroy = false`, because tearing the
    # API down would orphan the customer directory), and the configuration
    # outlives the state that described it. A later create does not collide on
    # a name the way a key ring does — it is refused outright, with
    # `Error 400: INVALID_PROJECT_ID : Identity Platform has already been
    # enabled for this project`, which is what stopped run 35523253654 after
    # it had already built 173 resources.
    "google_identity_platform_config",
)

# An object write is an overwrite, not a claim on a name: creating one over an
# existing object of the same key succeeds. These appear in the teardown's list
# only because an object has to leave state before the bucket holding it does,
# and reclaiming one would buy nothing.
CANNOT_COLLIDE = ("google_storage_bucket_object",)


def kept_addresses(modules_dir):
    """Every `type.name` the teardown takes out of state.

    Derived from the sources, never typed: any resource declaring
    `prevent_destroy = true` (a destroy that plans one aborts at plan time
    before touching anything else) and any bucket declaring
    `force_destroy = false` (it refuses while it holds objects this identity
    may not delete), plus every bucket object, which has to leave state with
    the bucket that holds it.
    """
    kept = set()
    for path in sorted(glob.glob(os.path.join(modules_dir, "*", "main.tf"))):
        last = None
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                header = re.match(r'\s*resource\s+"([^"]+)"\s+"([^"]+)"', line)
                if header:
                    last = f"{header.group(1)}.{header.group(2)}"
                    if header.group(1) == "google_storage_bucket_object":
                        kept.add(last)
                if last and (
                    re.match(r"\s*prevent_destroy\s*=\s*true", line)
                    or re.match(r"\s*force_destroy\s*=\s*false", line)
                ):
                    kept.add(last)
    return kept


def matches(address, kept):
    r"""Whether a plan address is one of the kept `type.name` pairs.

    Anchored on the address's own `type.name` followed by an instance key or
    the end, exactly as the teardown's `grep -E "\.${kept}(\[|$)"` is, so
    `bucket.evidence` cannot take `bucket.evidence_x` with it.
    """
    return any(
        re.search(r"(^|\.)" + re.escape(entry) + r"(\[|$)", address) for entry in kept
    )


def known(change, field):
    """A planned attribute, or None where this plan does not yet know it."""
    if change.get("after_unknown", {}).get(field) is True:
        return None
    value = (change.get("after") or {}).get(field)
    return value if value else None


def resolve(resource, project):
    """`(probe, import_id)` for one planned create, or `(None, reason)`.

    The identifier is built from the plan's *own* intended attributes rather
    than from a name written down here, so what gets reclaimed is by
    construction the object this apply was about to create. A name typed into
    this file could name something else and hand it to the apply.
    """
    kind = resource["type"]
    change = resource["change"]
    if kind == "google_identity_platform_config":
        # Answered before the name check below, because this resource has no
        # name to check: it is one configuration per project, addressed by the
        # project id alone. Asked for a `name` it does not declare, the check
        # deferred it as "the plan does not yet know what it would be called",
        # which reads as a timing problem and is really a category error — the
        # first version of this branch sat underneath that check and could
        # never be reached, and the apply failed again in exactly the way it
        # was written to prevent.
        #
        # The probe asks whether `identitytoolkit.googleapis.com` is enabled,
        # which is a proxy and is named as one: the configuration exists once
        # the API has been turned on and configured, and this asks only the
        # first half. It is used because `gcloud services list` is the same
        # surface every other probe here already depends on, and because the
        # failure mode of the proxy being wrong is a loud `terraform import`
        # error rather than a resource quietly built twice.
        return f"identity|{project}", project
    name = known(change, "name")
    if not name:
        return None, "the plan does not yet know what it would be called"
    declared = known(change, "project")
    if declared and declared != project and kind != "google_kms_crypto_key":
        return None, f"it is planned into project {declared}, not this one"
    if kind == "google_kms_key_ring":
        location = known(change, "location")
        if not location:
            return None, "the plan does not yet know its location"
        return f"keyring|{project}|{location}|{name}", f"{project}/{location}/{name}"
    if kind == "google_kms_crypto_key":
        ring = known(change, "key_ring")
        if not ring:
            return None, (
                "its key ring is not in state, so this apply is creating one, "
                "and a key under a ring that does not exist cannot exist either"
            )
        path = KEY_RING_PATH.match(ring)
        if not path:
            return None, "its key ring is not spelled as a resource path this can read"
        ring_project, location, ring_name = path.groups()
        if ring_project != project:
            return None, f"its key ring is in project {ring_project}, not this one"
        return (
            f"key|{project}|{location}|{ring_name}|{name}",
            f"{ring}/cryptoKeys/{name}",
        )
    if kind == "google_storage_bucket":
        return f"bucket|{project}|{name}", f"{project}/{name}"
    return None, f"this step has no way to ask Google whether a {kind} exists"


def imports(plan, project, kept):
    """Print one line per planned create worth reclaiming, and why for the rest."""
    if not SAFE.match(project):
        sys.exit(f"'{project}' is not a project id; refusing to build a command from it")
    reclaimed = 0
    replacing = []
    for resource in plan.get("resource_changes", []):
        actions = resource.get("change", {}).get("actions", [])
        address = resource["address"]
        kind = resource["type"]
        if "delete" in actions:
            # Not this step's business, and printed because it is the other way
            # an `up` half-builds an environment: a reclaimed resource whose
            # configuration has moved on is planned as a replace, and a replace
            # of a key or a bucket destroys the thing the teardown deliberately
            # kept. The operator reads these addresses in the plan above.
            replacing.append(address)
            continue
        if actions != ["create"]:
            continue
        if kind in CANNOT_COLLIDE:
            continue
        if kind not in NAME_SURVIVES_DESTRUCTION and not matches(address, kept):
            continue
        probe, identifier = resolve(resource, project)
        if probe is None:
            verb = "warn" if "no way to ask" in identifier else "defer"
            print(f"{verb}\t{address}\t{identifier}")
            continue
        unusable = [word for word in probe.split("|")[1:] if not SAFE.match(word)]
        if unusable:
            print(
                f"warn\t{address}\tits planned name has "
                f"{len(unusable)} component(s) no Google name contains"
            )
            continue
        print(f"import\t{address}\t{probe}\t{identifier}")
        reclaimed += 1
    for address in replacing:
        print(f"replace\t{address}\tthis plan would destroy or replace it")
    print(f"{reclaimed} address(es) to reclaim", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("addresses", "imports"))
    parser.add_argument(
        "--modules",
        default="infrastructure/terraform/modules",
        help="where the module sources are, relative to the working directory",
    )
    parser.add_argument("--plan", help="a plan rendered by `terraform show -json`")
    parser.add_argument("--project", help="the project the plan is against")
    arguments = parser.parse_args()
    kept = kept_addresses(arguments.modules)
    if not kept:
        # A parse that finds nothing reads exactly like a tree with nothing to
        # keep, and the caller would then apply over the remains it was written
        # to reclaim. No revision of this repository has ever had such a tree,
        # so an empty result means the modules are somewhere else.
        sys.exit(
            f"no module under {arguments.modules} declares prevent_destroy or "
            "force_destroy = false, which no revision of this repository has "
            "ever been true of: the parse is reading the wrong directory "
            "rather than finding nothing."
        )
    if arguments.command == "addresses":
        print("\n".join(sorted(kept)))
        return
    if not arguments.plan or not arguments.project:
        sys.exit("imports needs --plan and --project")
    with open(arguments.plan, encoding="utf-8") as handle:
        imports(json.load(handle), arguments.project, kept)


if __name__ == "__main__":
    main()
