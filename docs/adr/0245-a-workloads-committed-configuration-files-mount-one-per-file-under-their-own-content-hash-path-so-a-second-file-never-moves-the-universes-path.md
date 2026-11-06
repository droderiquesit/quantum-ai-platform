# ADR 0245: A workload's committed configuration files mount one per file under their own content-hash path, so a second file never moves the universe's path

- **Status**: **Accepted 2026-10-31 under the owner's delegation** (decision
  14 of `docs/blueprint/owner-decisions-2026-10-31.md`, the second delegated
  pass; decision 13 recorded that a second mounted file needs this decision
  first). The limits of that delegation hold here: no risk number, limit,
  threshold or limit set changes, nothing is sized, no billed infrastructure
  is added, and no paper-trading layer is touched. This decision changes
  where a file is mounted, not what any file says.
- **Date**: 2026-10-31
- **Rows**: R12-COMMERCE-5091, R12-COMMERCE-5092, R12-COMMERCE-5093 (and
  the mount R12-COMMERCE-5095 and R12-COMMERCE-5097 share).
- **Amends**: ADR 0036's amendment of 2026-10-13 (the committed universe is
  projected flat by a secret volume at its hash directory).

## Context

`modules/cloudrun` publishes a workload's committed configuration files
under one directory, `/etc/qip/<config_prefix>/`, where `config_prefix` is
the first sixteen hex characters of the SHA-256 of every file's own SHA-256,
joined in key order. Every revision reads its files under the name it
mounted, so a changed file is a new directory and never an overwrite.

That was right for one file and is wrong for two. The prefix hashes every
file, so adding a second file, or changing either, moves the directory of
both. The deep brain's universe is mounted from one single-item secret
(`qip-universe-catalogue-<env>`) at exactly that directory, and
`qip-acceptance`'s gitops suite holds the mount point equal to the
directory `QIP_UNIVERSE_PATH` names. A commerce plan mounted beside the
universe would therefore move the universe's path, the manifest's literal,
the secret's projection and the parity tests in one change, for a file that
has nothing to do with the universe. Decision 13 of 2026-10-31 stopped the
commerce breadth plan's patch on exactly that.

## Decision

1. **One directory per file, named by that file alone.** Each configuration
   file mounts at `/etc/qip/<prefix>/<file_name>`, where `<prefix>` is the
   first sixteen hex characters of the SHA-256 of the lowercase hex SHA-256
   of that file's bytes. Adding, removing or changing one file moves only
   that file's path.
2. **The universe does not move.** For a single file, the old joined form
   and the new per-file form are the same string: the join of one digest is
   that digest. So the universe's path is unchanged by this decision in
   every environment (dev's `QIP_UNIVERSE_PATH` stays
   `/etc/qip/931c5fe44d20d1d9/universe.json`, recomputed from the committed
   bytes), and no revision, secret projection or manifest literal for it
   changes.
3. **One volume per file.** Cloud Run mounts a volume at a path and two
   volumes may not share one, so each file is its own secret volume holding
   one item, projected flat at its own directory. The universe's volume
   keeps its name, `config-files`, and its secret; a further file's volume
   is `config-<key>`, from a secret named for that key and environment.
4. **Published the same way.** Each file is published to the workload's
   bucket under `<prefix>/<file_name>`, its own prefix, never overwritten,
   exactly as the one directory was.
5. **The parity rule generalises without loosening.** Every hash-named path
   a manifest mounts is still the name Terraform publishes the committed
   bytes under, now checked per file; a manifest whose literal differs from
   the per-file hash is refused, as the universe's is today.

## Consequences

- A second committed file (the commerce breadth plan first) can mount
  beside the universe with no change to the universe's mount, secret,
  manifest or tests.
- What still needs a person: creating and seeding each new file's secret,
  the per-environment variable naming the file, and the workflow seed list
  are protected paths and go in as patches.

## What it costs

- One volume and one secret per file rather than one for all of them.
- A reader must know the prefix is a hash of a hash, kept that way so the
  universe's existing path is the per-file path.

## What would make this wrong

- A workload that must read its files from one directory (a program that
  globs `/etc/qip/<prefix>/`). None does: every file reaches its process
  through its own `_PATH` variable.
- A Cloud Run limit on volumes per service low enough to bite. Each file
  costs one volume.
