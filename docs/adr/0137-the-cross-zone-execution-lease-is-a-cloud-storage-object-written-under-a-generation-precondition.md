# ADR 0137: The cross-zone execution lease is a Cloud Storage object written under a generation precondition

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It implements an existing trait over
  an existing in-tree adapter and admits no crate. The bucket it names is
  created only with an execution node, which no environment enables today,
  and its apply is the owner's like every other infrastructure change.
- **Date**: 2026-10-18
- **Extends**: ADR 0035 (one execution node per region), ADR 0113 §1 (Google
  APIs through the in-tree REST adapter over `rustls`, not an SDK).
- **Keeps**: ADR 0111 (Spanner is the fabric's P0/P1 store, not a general
  lock service), ADR 0003, RES-9167 (zero-gap failover is not promised).

## Context

`qip-edge-node`'s standby pair fences on a lease so two active owners never
exist (`qip-edge-node/src/standby.rs`). The `LeaseStore` trait is two
operations: `load`, returning the record and its version, and `swap`, which
writes only if the stored version is the expected one and reports a lost race
as `Ok(false)`. The one durable implementation, `FileLeaseStore`, is a
compare-and-swap over a file with an exclusive lock file, and its module
documentation is plain about the limit: "It is not a cross-zone store ... A
cross-zone pair needs a store with a real conditional write (an object
generation precondition, or a Spanner transaction — ADR 0111, 0113); that
store is not in this tree, and `main.rs` says so when a lease is configured."
R12-REFLEX-8069 is PARTIAL on that sentence. Terraform already passes
`fencing_lease_path` and `node_role` to both node templates.

The in-tree Cloud Storage adapter already issues conditional writes:
`qip-storage/src/gcp/storage.rs` sends `ifGenerationMatch=0` so a racing
creator is refused by the service, and `archive_names.rs` relies on it.

Locate them with
`grep -n 'pub trait LeaseStore\|pub struct FileLeaseStore\|pub struct LeaseKeeper' backend/crates/apps/qip-edge-node/src/standby.rs` and
`grep -n 'ifGenerationMatch' backend/crates/libs/qip-storage/src/gcp/storage.rs`.

## Decision

1. **A `GcsLeaseStore` implements `LeaseStore`** over one object. The
   object's generation is the version: `load` reads the object and its
   generation; `swap(None, next)` writes with `ifGenerationMatch=0`;
   `swap(Some(g), next)` writes with `ifGenerationMatch=g`. A `412
   Precondition Failed` is `Ok(false)`, a lost race; every other failure is
   an error, and an error is treated by `LeaseKeeper` exactly as today: the
   node does not believe it holds a lease it could not renew.
2. **Spanner is not used for this.** A regional Cloud Storage bucket is
   strongly consistent across the region's zones for object writes and
   generation preconditions, and needs no instance; a Spanner instance for
   one lease row would be paid capacity bought for a lock, and would put the
   fabric's database on the execution node's failover path.
3. **The bucket is the node module's, created with the nodes.** A regional
   bucket in the execution-node module, uniform bucket-level access,
   versioning off (the generation is the version), no public access, a
   lifecycle rule deleting nothing, and object-level IAM granting only the
   two nodes' service accounts `storage.objects.get/create/update` on the
   lease object. It is gated on `execution_nodes_enabled`, so nothing is
   created while no node exists.
4. **Expiry stays the holder's claim, bounded by skew.** The record carries
   its holder and expiry as today. A standby contends only after the stored
   expiry plus a configured clock-skew bound has passed on its own clock, so
   a holder whose clock runs slow is not deposed early; the bound is
   validated at start-up and refused if it is not shorter than the lease
   duration.
5. **`FileLeaseStore` stays for one host and drills.** The node's
   configuration selects the store by the lease path's scheme (`gs://` or a
   file path), refusing an unknown scheme; `main.rs`'s warning that a file
   lease is not cross-zone stays for the file case.

## Consequences

- A standby in a second zone can fence the primary, so a zone loss no longer
  leaves the choice between two active owners and no failover at all.
- Code slices left: `GcsLeaseStore` with tests against the storage adapter's
  test server (a won race, a lost race on `412`, an error that is not a lost
  race); scheme selection in the node's configuration; the bucket and its IAM
  in the execution-node module with a `tftest` or acceptance test that only
  the two node identities are granted; a two-keeper test that two processes
  sharing one object never both hold (REFLEX-8069).

## What it costs

- Each renewal is one HTTPS write to Cloud Storage: a few cents a month at
  the renewal interval, and a renewal now depends on the Storage API being
  reachable from the zone. A Storage outage in the region makes both nodes
  unable to renew, so the holder stops placing (fail closed) and nobody is
  promoted — an outage, not a split brain.
- A clock-skew bound adds that bound to every failover's duration.
- One more bucket, one more IAM binding pair, one more failure mode to read
  in an incident.

## What would make this wrong

- If the pair ever spanned regions, a regional bucket would put the lease in
  one of them and a region loss there would block the other; a dual-region
  bucket's write consistency would then have to be checked before reuse.
- If renewals needed to be faster than a Storage write's latency tail allows
  (sub-second leases), an object store is the wrong lock and the question
  becomes Spanner's or a consensus crate's, both owner decisions.

## Rows

R12-REFLEX-8069. It stays PARTIAL until its slices land with a mutated test
and a node is deployed to exercise it.
