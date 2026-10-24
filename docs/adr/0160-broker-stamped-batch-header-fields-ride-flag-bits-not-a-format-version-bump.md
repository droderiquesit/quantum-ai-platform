# ADR 0160: Broker-stamped batch header fields ride flag bits, not a format-version bump

- **Status**: **Accepted**, 2026-10-24, under the owner's delegation recorded
  in `docs/blueprint/owner-decisions-2026-10-24.md` ("Make all decisions for
  me", 2026-10-24). The delegation covers this design choice. It does not
  cover, and this ADR does not touch, any live path, billing, risk limit or
  protected file.
- **Date**: 2026-10-24
- **Amends**: ADR 0129 decision 4 ("the format version is bumped once for
  all four"). Decisions 1 to 3 and 5 of ADR 0129 stand.
- **Keeps**: ADR 0100 §1 (one codec, one framing), CONTRACT-048 (the
  by-stage header split), ADR 0109 (no dependency is added).

## Context

ADR 0129 has the broker stamp region, partition and the authenticated
principal into the batch header and the writer stamp provenance, and it
planned one format-version bump to carry all four. The fields land in
separate register rows and separate lanes: the partition (R12-FABRIC-8084)
was ready first; region, principal and provenance (R12-FABRIC-8022) were
not.

`qip-events`' codec refuses any batch whose format version it does not know
(`FORMAT_VERSION`), and it already reserves a flags byte that it refuses when
a bit it does not understand is set. A version bump made for one field would
either wait for the other three or be bumped again for each, and every bump
makes every batch on disk unreadable to a build that predates it, the
archive included.

## Decision

1. **Each broker- or writer-stamped header field ADR 0129 adds rides its
   own bit in the header's flags byte**, and is encoded after the
   previous-batch hash only when its bit is set. The format version stays 1.
2. **The field is inside the batch CRC**, and so inside the next batch's
   previous-batch hash, exactly as the version-bump design would have put
   it. A flipped bit in it is corruption.
3. **A batch without the bit decodes with the field absent** (`None`), never
   defaulted. Every batch written before the field existed is such a batch.
   This is ADR 0129 decision 3's "explicitly absent" read, reached without a
   version boundary.
4. **A bit outside the known set is still refused.** A reader never guesses
   at a field it cannot parse.
5. The partition is the first such field (`FLAG_PARTITION`, 2026-10-24).
   Region, principal and provenance take the next bits when R12-FABRIC-8022
   lands, each with the same round-trip, absent-on-old-batch and
   flipped-bit tests.

## Consequences

- Old batches and the archive stay readable by every build from this one
  on, with no migration.
- **A rolling upgrade is consumers first.** A build that predates a flag
  refuses a batch carrying it, by decision 4. Every reader (`qip-ledgerd`,
  `qip-edge-node`, `qip-transport` consumers, a managed `fabricd` catching
  up) must run a build that knows the bit before any broker stamps it. No
  environment is deployed today, so nothing has to be sequenced yet.
- The flags byte has eight bits. Four are spent by ADR 0129's fields;
  anything beyond eight more header fields would need the version bump this
  ADR defers.

## What would make this wrong

- A reader that must read batches from a build *newer* than itself
  (forward compatibility). Decision 4 refuses those by design; if that ever
  becomes a requirement, the format needs a length-prefixed extension block
  rather than flags.
