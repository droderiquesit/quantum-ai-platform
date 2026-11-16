# ADR 0612: A cell records the two regional episode kinds it can name without a figure, and not the detectors that would need one

- **Status**: Proposed, 2026-11-14, by wave b52 lane 06 for the owner's review.
- **Date**: 2026-11-14
- **Amends**: ADR 0207 decision 3, "No new recording seam". That bullet kept
  `Microstructure` and `VenueBehaviour` unrecorded.
- **Keeps**: ADR 0003 (paper trading), ADR 0109 (no dependency),
  `Cell::record_episode` private, and the delta as a view.

## Context

R12-REFLEX-6012 names microstructure drift, queue behaviour and venue
anomalies as episodes a region shares. ADR 0207 carried the four kinds the
cell already recorded and declined a seam for the other two, because a kind
nothing records is not a kind the wire invents. It did not rule out a seam
that already exists.

## Decision

Two existing seams record, each once, each with no new threshold:

1. `Microstructure` where the cell discards a book the venue left crossed at
   the end of a batch outside an auction (`Cell::apply_batch`). The cell
   already judges that fact; the episode names venue, instrument and the
   amount crossed.
2. `VenueBehaviour` where `Cell::review_venues` first finds a venue in
   quarantine on its reject rate. The cell already journals that entry; the
   episode is recorded on the same entry, once, not on every pass it holds.

## What is deliberately not done

- **No drift, queue or fee/latency-profile detector.** Each needs a
  tolerance or window nobody has given, and a lane does not choose one. The
  requirement's "drift" and "queue behaviour" stay unrecorded and the row
  stays open on them.
- **No change to the wire, the store or the bounds.** Both kinds already
  cross as `RegionalEpisode`.
- **Nothing decides from an episode.** The store is still read by nothing
  that decides.

## What it costs

Two of the six kinds now have a recording site, so the centre can answer
"which venue left rotation in this region" and "which book was corrupt".
Drift and queue behaviour stay unrecorded, so the requirement's row stays open
on them and a region cannot yet share those episodes. Each recorded episode
takes one slot in the 128-slot retained ring (`MAX_RETAINED_EPISODES`), so a
venue that enters quarantine or a run of crossed books can push older episodes
out sooner; how often either fires in practice is unmeasured. The two
recordings add code inside seams that already exist: `Cell::apply_batch`,
reached from `Cell::on_bytes` and from the pass, and `Cell::review_venues`,
called on the pass. Delivery is still gated on `QIP_MESH_CELLS` and a
deployed execution node, and none is deployed.

## What would make this wrong

An episode filed at a seam that fires every pass would flood the 128-slot
ring and push out the four kinds ADR 0207 already carries. Both seams
fire on a state entry, and the tests assert one episode, not one per pass.
The record should also be revisited if the centre needs drift or queue
episodes to act on, since this decision leaves those two kinds with no
recording site, or if a region's quarantine entries or crossed-book discards
turn out frequent enough to displace the older episodes from the ring.
