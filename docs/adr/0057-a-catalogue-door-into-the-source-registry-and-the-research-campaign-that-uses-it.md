# 0057 — A catalogue door into the source registry, a bounded reference ledger with a consequence, and the research campaign that uses them

**Status:** *accepted*, 2026-09-12; **amended in place the same day** after
two independent reviews (security-engineer, code-reviewer) of the seven
commits that implemented it found one blocking defect and eleven others.
Every finding is fixed in the commits that follow `93c4c7b`, and this record
is corrected where it stated something those commits made false: the manifest
is on the log only, the records have their own topics, the ledger outlives
the process, the gate is the only way in on the wire as well as in code, a
door refusal is a round's outcome, and rule 31's hold can open. The costs
section says what each of those costs.

**Relates to:** blueprint §22.1 (Retention Classes), §22.2 (Sufficient
Statistics), §22.3 (Data References), §22.4 (Fetch-on-Demand for Research),
§56.3 rules 30 and 31, §56.4 rules 33 and 35
(`docs/architecture/algorik-blueprint-v10.1-source.md`); ADR 0056, which
built `DataReference` and `FetchCampaign` and scored both `PARTIAL`; ADR
0034 (the connector catalogue); ADR 0003 (paper trading); and
`.claude/rules/domains/data-and-streaming.md`'s licensing-before-use and
bounded-retention rules.

**Supersedes in part:** ADR 0056's "What would make this wrong" bullet that
no constructor may build a `DataReference` from a source that never went
through `DataFinder::assess` — one may now, through a second door that runs
its own gate, and this record says which gate and why it is not weaker.

---

## Context

ADR 0056 built the type §22.3 asks for and the campaign §22.4 asks for, and
commit `bbf31c8` then established that neither had a production caller and
could not honestly get one by wiring alone. `DataReference::of` took a
`&RegisteredSource`, which only the discovery pipeline produces after
probe → classify → score → route; the four shipped connectors (Coinbase,
Alpaca, Frankfurter, Kalshi) are admitted through a different, already-real
path — `qip_data_finder::admission`, whose gate returns a
`LicensingDecision`, never a `RegisteredSource`. Building a
`RegisteredSource` for a connector would have meant fabricating probe
evidence, a scored routing and a lineage that no connector ever had. That
commit named two honest designs and made neither. §22.4 inherited the
blocker, and carried a second: a continuously running connector is not a
bounded research run, and forcing one poll per campaign open/close would
misrepresent it as one.

The owner has now decided that §22.3 and §22.4 are to be reached, with a
real non-test call path, and that the foundations §22.4's mitigations lean
on — §22.1's retention class and fallback series, one §22.2 sketch with a
declared error bound — are built rather than deferred. What is not
authorised is any new dependency, any weakening of the licensing gate or a
paper-trading control, fabricated evidence, or an unbounded buffer.

## Decision

Six things, in the dependency order they were built and committed.

### 1. The second door: `AdmittedSource`, an unforgeable `LicensingDecision`, and `SourceOrigin`

A catalogue-admitted connector enters the finder's reference machinery as an
`AdmittedSource` — a distinct type, not a `RegisteredSource` — built by one
constructor, `AdmittedSource::from_decision(&LicensingDecision,
&SourceManifest)`. It carries exactly what happened to the source: the
catalogue's licensing evaluation (licence, class, instant) and the
manifest's declared category and schema contract. It refuses a decision and
a manifest naming different sources, a class disagreement, a `Synthetic`
class, and a manifest that declares no category.

`LicensingDecision` gains a private zero-sized `GatePassed` field. Every
other field was public so a banner could read it, which meant any code
could write one, and a type gated on "a decision exists" would have been
gated on nothing. `grep -n 'LicensingDecision {'
backend/crates/services/qip-data-finder/src/admission.rs` finds the single
construction site, inside `admit_from_registered`, after every usage
question has been answered `permitted`.

`DataReference` records which door its source came through in a new
`SourceOrigin`: `Discovered` (the ADR 0056 path, unchanged, still refusing a
source with no category), `CatalogueAdmitted` (this door), and `Generated`
— a stream this platform produced itself, gated on the descriptor's
licensing class being `Synthetic` and carrying **no** §7.6.1 category. The
precedent is `qip_financial::manifest::SourceManifest::generated`, which
files generated text under a distinct source so nothing downstream can
mistake it for something a vendor could be asked to serve again. The
category precondition ADR 0056 stated therefore still holds for every source
a vendor could serve again, and is answered structurally by the origin for
the one kind nobody could. `DataReference::category()` returns `Option`
accordingly.

`SourceCategory` moves down to `qip-financial` and is re-exported from the
finder under its old path. The crate that ships the manifests sits below the
finder on the dependency edge, so an enum kept in the finder could never be
written into one, and a second enum in the ingestion crate would be two
definitions of one table. The finder keeps `ContentSignal` and the
classifier — the parts that decide.

Each shipped manifest declares its category: Coinbase, Alpaca and Kalshi
`marketplace`, Frankfurter's ECB rates `government_and_trade`. Kalshi is
deliberately not `resolution_source`: the connector decodes yes-price quotes
on open contracts, which is price history on a venue, and the blueprint's
resolution source is "the official pages that determine event outcomes" —
what those quotes settle against, not what they are.

### 2. The digest, taken where the bytes exist

`ConnectorRuntime::ingest` is the one seam holding a vendor's raw body, the
locator it was fetched from, the decoded events' own instants and the
subjects they map to at once. It records a `FetchDigest` there — source,
locator, SHA-256 and length of the body, poll horizon, the period the events
span, the source's own keys (`symbols`, for a reconciliation against the
vendor), this platform's ids for what the mapped records are about
(`subjects`, what the reference ledger and a campaign join on — the first
version carried only the vendor's row keys, and a campaign asking by
`ObjectId` never found a connector's reference), the schema version — and
carries it out on the `PollReport`. The digest is taken over every decoded
event, withheld and duplicate ones included, each mapped once so a re-served
table names the same subjects whether or not the dedup window admitted
anything. A body decoding to no events, or mapping to no subject, yields no
digest and changes nothing else about the poll. The body itself is never
retained, and the digest does not deserialise: its one constructor hashes a
body it was given.

The bridge hands the digest to the root's reference hook *between the poll
and the checkpoint commit* — `ConnectorFeed::poll_referencing` — and both
roots pass `Platform::reference_fetch` through it. A refusal there unwinds
the runtime (cursor, dedup window, ingest counters), records nothing and
commits nothing, so the next poll re-fetches the same extent. The first
version committed the checkpoint before the root referenced the fetch, and a
refused reference dropped a batch the connector had already resumed past.

### 3. The bounded ledger, and what a revision costs

`qip_data_finder::ledger::ReferenceLedger` keys every reference on the
extent it describes (source, locator, period) and compares the next
reference to the same extent hash to hash against the last, keeping the
newer. Bounded by count on both axes — 4,096 references, 1,024 revision
records by default — refusing zero, evicting the oldest extent by insertion
order rather than last use, and counting what it evicted. `assess` is the
finding and `record` the mutation, split so a log can hold the finding
first.

`Platform::reference_fetch` builds the reference from a digest through
`DataReference::from_digest`, which takes the digest's hash rather than
bytes: a `FetchDigest` has one constructor that hashes a body it was given,
so a hash can only reach the ledger by having been taken over real bytes. It
refuses a digest from a source the platform holds no `AdmittedSource` for.
The log is written before the ledger moves, every time: the reference as a
`DataReferenceRecorded` record (Sense group, idempotent on extent and hash),
and on a revision, a `SourceRevisionDetected` record under its own
permanently retained Learn-group topic, then a `ResearchCampaignFlagged`
record for every campaign already closed on the log whose manifest read the
withdrawn bytes — the backtest that used the original, found by joining the
revision against the closed manifests, not against whatever campaign
happens to be open — then `qip_data_revisions_detected_total{origin}` and
`qip_research_campaigns_flagged_total{origin}` move, and the revision stays
in the queue `Platform::revision_covering(source, subject, period)` answers
from. Each record carries an idempotency key and `Platform::journal_once`
consults the log for it, so a fact journaled twice is one record.
`Platform::new` rebuilds the ledger from the log in order, so a restart
resumes knowing what it referenced and what it found revised. The first
version filed the revision under `DataQualityFailed` — not permanently
retained, and already carrying another body — and the ledger died with the
process.

Both composition roots reference the fetch through the bridge's hook before
`Platform::observe`, and the platform's admission is derived from the feed's
one seam below the root — `Api::with_feed`, `qip-fastbrain`'s `node::step`
(copied whenever the two differ, as the API's re-admission route does) — on
the precedent of `ConnectorFeed::journal_to`.

### 4. §22.1's classes and the fallback series; §22.2's one sketch

`RetentionClass` is the nine rows of §22.1's table (nine, not the ten
`docs/DELIVERY-STATUS.md` once counted), each answering with the row's own
policy. `FallbackSeries` is the one row that needed a structure: daily bars
per instrument under three stated bounds — three years behind the newest
bar, 1,100 bars per instrument, 512 instruments — refusing zero on every
axis, refusing a non-daily bar, refusing a new instrument past the bound
rather than evicting another's insurance. The kernel feeds it from
`Platform::observe` for daily bars only.

`qip_numerics::sketch::CountMinSketch` is built from an `ErrorBound(ε, δ)`
and states the guarantee exactly: never below the true count, above it by at
most `ε·N` with probability `1 − δ`. `ErrorBound::tolerable_for(total,
tolerance)` is the consumer's refusal, and `ErrorBound::new` refuses a pair
whose counters would exceed `MAX_COUNTERS` (65,536, half a megabyte) or
overflow, on the wire as in code, so the module's "kilobytes" is a number
rather than a hope. `SketchedStatistic::new` refuses an estimate above the
total, which no sketch produces.

### 5. §22.4's caller: the deep brain's learning assembly as a campaign

`qip_deepbrain::campaign::assemble` wraps the learning desk's window
assembly: resolve the stream to its door — admitted, or generated; anything
else is `Assembly::RefusedAtDoor`, a fact about one subject and one round
that the round line carries and `qip_research_campaigns_refused_total{gate}`
counts, never an error that leaves the node loop (the first version made it
one, and the deep brain stopped on its first due learning round over an
undeclared replay); take the subject's own history, or §22.1's fallback
series when the stream no longer holds enough; serialise the window and
reference it through that door; record it on the platform's ledger (hash
verification across rounds, with the kernel's consequence); fetch it into a
`FetchCampaign` under a stated `CacheBound`; flag the manifest entry only if
this campaign read the withdrawn bytes (`RevisionRecord::contradicts` — the
first version flagged the campaign that had just fetched the *corrected*
bytes, and nothing named the closed one that had used the original); count
bars per subject with the sketch and attach the statistic and its bound to
the manifest, refusing the fit when the declared error at this volume
exceeds five percent of the desk's minimum; read the window **back out of
the cache** for the desk to fit on; assess concentration over this stream
plus every ledger source naming the subject, each with its door; close, and
journal the manifest as a `ResearchCampaignClosed` record under its own
permanently retained topic — the only place it goes. `EvolutionEngine::
maybe_learn` calls it on the learning cadence, from `qip-deepbrain`'s node
loop — the non-test caller.

A replay may name the shipped connector it was recorded from
(`# recorded-from:`); the root runs that source through
`StandingAdmission::open`, gives the replay the source's name and class, and
the engine re-asks the gate on every due round and copies the answer into
the platform before the door is resolved. That is the one way a deep brain
without a live vendor call researches through the catalogue door, and it is
what lets a second vendor back a subject.

### 6. Rule 31's consequence: promotion past validation is gated

In `EvolutionEngine::turn`, the concentration verdict is taken once per
round and applied at the one seam where "promoted past validation" is a
thing the node does — the holdout gate's promotion to the ladder's first
rung. A candidate on a subject backed by fewer than two independent
**vendors** is registered (the search happened, the trial is charged) and
**held at `Candidate`, unjudged**, counted as `held_back` on the round
summary with the verdict's own words. Only the vendor doors count —
`SourceOrigin::is_independent_vendor` — and a generated stream is reported
on the verdict as excluded: the rule is about a vendor withdrawing access,
and this platform cannot withdraw access from itself, so two synthetic
streams are not two sources of anything (the first version counted them).
In the shipped deployment the deep brain's one stream is the synthetic
exchange, which counts for no vendor at all, so every round is held back.
That is the rule working, and it is why the choice fell on promotion rather
than on training: a fit is evidence and may be gathered on one source;
promotion is a decision and may not.

## Which of §22.4's five mitigations this closes — read exactly

| Mitigation (§22.4's table) | Status | Where |
|---|---|---|
| Source revises history after use | **Built, with a consequence** | `ReferenceLedger::assess` → `Platform::record_reference`: log record under its own topic, the closed campaigns that read the original named on the log, metric, `revision_covering` flag; `campaign::assemble` flags its own manifest only where it read the withdrawn bytes |
| Research is slower than a local copy | **Built, on the path** | `FetchCampaign` under `CampaignConfig::cache`; the window is read back from the cache |
| Regulatory demand for data not retained | **Built, on the log** | `ResearchCampaignClosed` on the log under its own permanently retained topic, and nowhere else |
| Vendor withdraws historical access | **Built; shut in every shipped deployment** | `assess_concentration` over vendor doors only, gating promotion in `EvolutionEngine::turn`; `FallbackSeries` fed from `observe`, drawn on by `assemble` and recorded on the manifest. The gate opens for two admitted replays from two vendors and for nothing shipped — see the costs |
| Sketch or reservoir error affects a model | **Built** | `CountMinSketch` with a declared `ErrorBound` on the manifest; `assemble` refuses when `tolerable_for` says no |

Five of five, each with a production call path, each with a test that was
mutation-verified.

## What was checked before committing to it

- **No new dependency.** Every addition uses `serde`, `serde_json` and
  workspace crates; `qip-data-finder` names `qip-market` and `qip-numerics`,
  which it reached transitively before. `./scripts/check-dependencies.sh`
  reports 11 third-party packages, all permitted, unchanged.
- **The licensing gate is the only way in — on the wire as well as in
  code.** `AdmittedSource` takes a `LicensingDecision`; the decision cannot
  be constructed outside `admit_from_registered` (`GatePassed`);
  `admit_from_registered` refuses an ambiguous posture and a research-only
  licence; `from_digest` refuses a digest naming another source;
  `Platform::reference_fetch` refuses a source it holds no admission for;
  `of_generated` refuses any class but `Synthetic`; `assemble` refuses a
  stream that is neither; and `AdmittedSource` and `FetchDigest` do not
  deserialise, because a `Deserialize` derive is a second constructor that
  takes any caller's word — the first version of this record claimed the
  gate was the only way in while both types derived it, and a
  `compile_fail` doctest now pins the absence. `ReferenceLedger` and
  `FallbackSeries` do not deserialise either; `ErrorBound` and
  `SketchedStatistic` deserialise through their constructors. Each refusal
  has a test and each test was mutated.
- **The paper-trading boundary is untouched.** No file under
  `qip-risk-engine`, `qip-execution-engine`, `qip-capital`, `qip-edge` or
  `infrastructure/terraform` changed; the only promotion touched is the
  candidate-to-first-rung step, which holds no capital, and the change only
  ever holds a candidate back.
- **Every buffer is bounded and stated.** Ledger 4,096/1,024; fallback
  3 years / 1,100 / 512; campaign cache 4 entries, one hour; sketch
  `⌈e/ε⌉ × ⌈ln 1/δ⌉` counters and at most `MAX_COUNTERS` of them; digest
  key and subject sets bounded by the manifest's batch cap; the log's
  idempotency index bounded by the log's capacity; the fallback refusal
  set bounded by the instruments observed.
- **No new environment variable**, so no Terraform half and no change to
  `manifest_wiring.rs`'s allowlist. The replay's `# recorded-from:` header
  is a line in a file the existing variable already names.

## Alternatives considered and rejected

**Build a `RegisteredSource` for a shipped connector.** Rejected for
`bbf31c8`'s reason: it carries probe evidence, a scored routing and a
lineage that never happened to a connector, and defaults there read as
findings.

**Narrow `DataReference::of` to the id and category it reads**, taking any
caller's word for both. Rejected: a category and an id with no gate behind
them are exactly the "second admission system" the correction warned of.
The door instead takes the gate's own decision, made unforgeable.

**Kalshi as `resolution_source`.** Rejected on the blueprint's own
definition; see §1.

**One connector poll per campaign open/close.** Rejected, as `bbf31c8`
already had: a permanent feed is not a bounded run. The campaign is the
learning round, which is one.

**A one-minute fallback series**, as §22.1's row literally says. Deviated
to daily bars and said so: a minute series over three years is a million
bars per instrument, and a platform with no deployed collector has no
honest use for a million-row insurance policy it cannot yet be shown to
need. The bounds are constants; the interval is a declaration.

**Gate the fit itself on concentration.** Rejected: a fit is evidence and
may be gathered on one source; the rule is about promotion past validation,
and the ladder's first rung is that seam.

**Admit the source to the platform in each root's `main.rs`.** Rejected
after the API's own feed test refused every cycle: a rig is a second
composition path, and the admission lives at the seam every path passes
through instead.

**A weighted reservoir sampler as the one sketch.** Not rejected on merit;
the count-min sketch was chosen because a frequency with a declared `(ε,
δ)` bound is the one §22.2 names "bounded error" against and the one a
consumer can refuse on a number.

## What it costs

**Promotion past validation is held in every shipped deployment, and the
reason is stated rather than hidden.** Three facts, each of which alone
holds it. The shipped deep brain's one stream is the synthetic exchange, a
generated stream that counts for no vendor at all, so every round is held
back at `Candidate`, unjudged, and the round line says so. With the four
shipped connectors no subject has two independent vendors — crypto is
Coinbase alone, equities Alpaca alone, FX Frankfurter alone, prediction
Kalshi alone (and Kalshi and Alpaca are refused by the catalogue until
their terms are read) — so no connector-fed deep brain can count two either,
until the catalogue gains a second vendor for a subject. And a single
deep-brain process feeds one stream, so even then both vendors' references
reach one ledger only through two learning streams, which no deployment
shape provides today. The gate *can* open without a live vendor call — two
replays recorded from two admitted connectors, each through its own gate,
proven end to end in `two_admitted_replays_from_two_vendors_lift_the_hold_
and_one_does_not` — and in every shipped deployment it is shut. That is the
rule working; a deployment that wants promotion needs a second vendor for
the subject and a process fed both, which is what the rule exists to demand.

**A connector-fed deep brain fails closed per subject, not per process.** A
stream the door refuses — an undeclared replay, a replay whose named source
the catalogue refuses, a standing admission that has lapsed — is a learning
round with no campaign, on the round line and counted, every round, for as
long as the stream is refused; the node keeps cycling and fits nothing. The
first version stopped the process on the first such round, which read as a
crash and was the door working.

**The ledger is rebuilt from the log on every restart, and only from what
the log retains.** Every reference is a `DataReferenceRecorded` record
before the in-memory ledger moves — one more log record per delivered poll,
Sense group, evictable — and every revision a permanent one, so a
restarted process resumes knowing what it referenced and what it found
revised. Two consequences are stated: an extent re-fetched unchanged is
held, after a restart, at the instant it was *first* referenced rather than
last (the record is idempotent on extent and hash), which is the earlier and
more conservative `used_at` for a later revision to flag against; and a
log whose reference records the capacity bound has evicted rebuilds a
ledger that has forgotten those extents, which is the same forgetting the
ledger's own count bound already does, on the log's schedule instead of the
ledger's. A ticker polled every two seconds writes a reference record every
two seconds; at the log's default capacity of a million that is three
weeks of records before the oldest are evicted.

**Every learning round serialises and hashes its window and appends a
record to the log.** A few hundred kilobytes of serialisation and one
SHA-256 per round, on the research node, off the cycle's budget.

**A generated reference has no category, and no standing.** A reader asking
"what kind of source" of a synthetic stream gets "this platform", from the
origin, and nothing from the category; and it counts for nothing toward
rule 31, however many of them name a subject.

**A refused reference re-fetches the same extent on the next poll, for as
long as the platform refuses it.** The connector is unwound, the checkpoint
stays, and the next poll after the manifest's interval fetches the table
again; a platform that refuses every time — a root that never admitted the
source — fetches the same table every interval and delivers nothing, which
is loud on the cycle line and is the intended shape of a root that skipped
a step. The alternative, committing past records nobody reasoned over, was
silent loss.

**A replay's `# recorded-from:` header is a claim by the file's author.**
The platform verifies that the named source's licence exists and is
granted, not that the bytes came from that vendor — the same trust it
already extends to a `with_licensing` call, and the same standing as a
manifest's declared category: a claim by this platform's authors, reviewed
like the file.

**The platform holds a second copy of the feed's admission.** Two records
of one decision, derived one from the other at a stated seam; if the seam
is bypassed, the platform refuses rather than guesses, which is the cost
paid in the API's feed test before the seam moved.

**The sketch on a one-subject campaign is exact**, and its bound is formal
rather than load-bearing today. What it earns its place with is the
refusal path and the memory bound a many-subject campaign would draw on.

**Eviction by insertion order** means a re-fetched extent ages out on the
ledger's schedule, not its own; the trade is stated in the ledger's doc.

## What would make this wrong

- **A third door with no gate.** A constructor for `AdmittedSource`,
  `LicensingDecision` or a `DataReference` origin that takes a caller's word
  for the licence would unmake §1. `grep -n 'LicensingDecision {'` must keep
  finding one site, and `AdmittedSource` must keep failing to deserialise —
  a `Deserialize` derive is such a constructor, and this record shipped with
  one until the review found it.
- **A category declared on a manifest that is not what the source is.** The
  declaration is a claim by this platform's authors, reviewed like the
  manifest; the manifest suite asserts each by name.
- **A deployment that admits a second source to lift the concentration hold
  without it being independent** — a replay of the same vendor's data
  declared as recorded from another is not a second vendor, and the ledger
  counts source ids and doors, not provenance. The file's author is
  answerable for the declaration.
- **A record here journaled under a shared topic again.** Every body in
  `qip_kernel::references` has its own `Topic`; one filed under
  `LearningCompleted` breaks `Platform::journal_entries` on the first close,
  and one filed under a Sense-group topic other than `DataReferenceRecorded`
  is evicted with the observations.
- **A minute-bar deployment that needs the fallback series** and finds it
  empty, because only daily bars are retained. The row's interval was
  deviated from deliberately; a need for finer insurance reopens it.
- **A campaign that assembles many subjects** and finds the cache bound of
  four or the sketch's tolerance wrong for it. Both are constants in
  `qip_deepbrain::campaign`, chosen for one subject per round.
