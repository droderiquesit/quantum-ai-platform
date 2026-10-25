# ADR 0180: Connector health is scored, world documents are re-fetched and never copied, and the scout records every first-pass verdict

- **Status**: **Accepted**, 2026-10-25, decided under the owner's delegation
  of 2026-10-24 ("Make all decisions for me , I want the end result to have
  a full system"), recorded in
  [`docs/blueprint/owner-decisions-2026-10-24.md`](../blueprint/owner-decisions-2026-10-24.md).
  That delegation does not cover a live order, new billed infrastructure, a
  change to a financial risk limit or a weakened guard, and nothing below
  needs any of them.
- **Date**: 2026-10-25
- **Amends**: [ADR 0157](0157-a-source-enters-only-as-a-committed-licence-record-that-quotes-the-terms-it-was-read-from-a-descriptor-never-polls-and-vendor-choice-is-billed.md)
  §8 (connector health) only. §4 (a descriptor never polls) and §5 (no crawl
  builds candidates) stand, and this record says so explicitly for the two
  rows that asked for them to change.
- **Rows**: R12-DATA-1103, R12-DATA-2024, R12-DATA-1101, R12-DATA-1104,
  R12-DATA-8018.

## Context

Five sensing rows waited on a decision:

- **DATA-2024** asks that a source be scored on reliability from observed
  availability, errors and latency. ADR 0157 §8 decided that connector
  sources do not enter `DataFinder`'s registry and that "nothing scores"
  from the connector's measured health. The requirement text names a score,
  so under part 4 of the completion bar the row cannot close while §8
  stands.
- **DATA-1103** asks that the scout's access-rights validation be recorded in
  the event log. `Platform::assess_sources` journals only
  `SourceLifecycleChanged` for a source already registered, so a candidate
  refused on its first pass, for its licence or its robots rules, leaves no
  record.
- **DATA-1101** and **DATA-1104** ask the scout to search continuously and to
  create adapters. ADR 0157 §5 and §4 decided the opposite: candidates are
  operator-written, and no code builds a connector from a registered source.
- **DATA-8018** asks that historical world analysis re-fetch by manifest and
  never copy corpora, and that "a cached copy path must not exist". The
  platform keeps two decided captures that read like copies: the tick lake
  (venue market data under a retention right, ADR 0113 and ADR 0151 §8) and
  the recorded tapes and replays the deep brain can be pointed at.

## Decision

1. **A connector's polls are scored (DATA-2024; amends ADR 0157 §8).** Each
   connector arm records one observation per poll: served, failed or timed
   out, with the transport's elapsed time and the freshest payload instant.
   Observations are kept in a bounded per-source window. A reliability score
   is computed from that window with the same scorer `DataFinder` uses for
   discovered sources, so one formula grades both. Connector sources still
   do not enter the finder's registry; that half of §8 stands, because a
   connector is admitted by licence and a second probe beside the poll would
   double the traffic. The score is reported on the arm's status and journaled
   when it moves by more than a stated step. **It routes nothing** in this
   record: no source is dropped, throttled or preferred on it. A score that
   decides nothing yet is still a measurement a person can check, and it is
   the input a later decision about routing would need.
2. **Every first-pass verdict is journaled (DATA-1103).** When
   `Platform::assess_sources` assesses a candidate for the first time, the
   verdict — admitted, or refused with the licence, robots or manipulation
   reason by name — is journaled once, on the existing source topic, before
   any registration. A refusal nobody can see afterwards is a refusal nobody
   can audit. Enabling the discovery cadence in a committed environment stays
   off: the probes reach third-party publishers and need egress routes that
   cost money and are not built, which is outside the delegation.
3. **No continuous search stage and no generated adapters (DATA-1101,
   DATA-1104).** ADR 0157 §4 and §5 stand. A source a crawler found, or an
   adapter generated from a descriptor, would put data in front of the desk
   whose terms nobody read, which is the failure ADR 0157 exists to prevent.
   Both rows stay PARTIAL, citing this record: the requirement asks for
   something the platform deliberately does not do, and the owner may still
   change the requirement.
4. **"Never copy" is a rule about world documents (DATA-8018).** The clause
   applies to documents and pages a world source published: news, filings,
   statistics releases. For those, analysis of the past re-fetches the
   document through its manifest and checks the fetched hash against the
   manifest's (`qip-data-finder` `refetch.rs`, `RevisionCheck`); no store
   keeps the body. It does not apply to venue market data kept under a
   retention right in the tick lake, nor to recorded tapes of the platform's
   own or licensed feeds: those are retained deliberately, under ADR 0113,
   ADR 0151 §8 and the retention policy, and re-fetching a tick is not
   possible. The remaining code is a re-fetch triggered when a replay touches
   a past document, rather than only on the cadence
   (`QIP_DEEPBRAIN_REFETCH_EVERY`).

## Consequences

- DATA-2024 becomes closable by code: time each connector poll, keep the
  window, score it, test that a flaky connector scores lower than a stable
  one, and mutation-verify that ignoring the observations fails the test.
- DATA-1103 becomes closable by code: journal the first-pass verdict and test
  that a robots refusal is in the event log. Its default-path half still
  waits on the discovery cadence, which stays off.
- DATA-1101 and DATA-1104 stay PARTIAL by decision.
- DATA-8018's scope is settled; the replay-triggered re-fetch is code.

## What it costs

- A per-source window of poll observations in each connector arm, bounded,
  and one journal record per material score change.
- A score nobody routes on can look like decoration. It is kept anyway,
  because the alternative is a routing decision made later with no history
  behind it.

## What would make this wrong

- If connector polls were too frequent for a per-poll observation to stay
  cheap, the window should sample instead of recording every poll.
- If a licensed source's terms permitted keeping document bodies and the desk
  needed them, decision 4 would need a licence-scoped exception, recorded the
  way ADR 0157 records terms.

## Not decided here

- Routing on the reliability score.
- Turning on the discovery cadence or adding scout egress routes.
- Any vendor or licence.
