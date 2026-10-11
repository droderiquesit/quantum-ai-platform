# modules/ledgerd

`qip-ledgerd`, the ledger writer of ADR 0100 §2, on one Compute Engine
machine: its own identity (log and metric writing only), a persistent
archive disk attached as `qip-ledger`, a stable internal address in the
`ledger` trust zone, no external address, shielded boot, and a startup script
that refuses to run unless the archive is mounted from its own disk and the
boot image carries `qip-ledgerd.service`.

Instantiated by the root's `fabric.tf` behind the `ledgerd` object, which is
null in every environment.

## Why one machine and not a group or a Cloud Run service

The ledger is the sole writer of its chain. A managed instance group's
surge creates a replacement before retiring the original, and a Cloud Run
rollout runs two revisions side by side: either is two writers. One machine
is the honest shape.

## What is not here

- **A boot image.** Nothing bakes one; `image.yml` bakes the execution
  node's. Until an image carrying the binary and its unit exists, `ledgerd`
  stays null, and the module refuses anything but a self-link to one image.
- **The fabric address.** `qip-ledgerd`'s composition root does not yet read
  a broker address from its configuration (`main.rs` holds a placeholder),
  so this module writes none. When the binary reads one, the root passes
  the broker's SRV name from `fabric.tf`.
- **The paper-trading fence.** ADR 0106's refusal of a `simulated=false` fill
  is the binary's. There is no setting here that could relax it, and there
  must not be one.

`terraform test` from this directory plans the module with a mocked provider.
