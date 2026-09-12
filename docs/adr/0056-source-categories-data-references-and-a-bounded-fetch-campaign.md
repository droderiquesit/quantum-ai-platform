# 0056 — Source categories, data references, and a bounded fetch campaign

**Status:** *accepted*, 2026-09-12.

**Relates to:** blueprint §7.4 (Classify stage), §7.6.1 (Source Categories),
§22.3 (Data References), §22.4 (Fetch-on-Demand for Research)
(`docs/architecture/algorik-blueprint-v10.1-source.md`), `docs/DELIVERY-STATUS.md`
rows for §7.4, §7.6.1, §7.6.3, §22.1, §22.2, §22.3, §22.4, and
`.claude/rules/domains/data-and-streaming.md`'s bounded-retention and
licensing-before-use rules.

**Does not touch:** `backend/crates/runtime/qip-kernel/**`,
`qip-learning-engine/**`, `qip-optimization-engine/**`,
`qip-portfolio-engine/**`, `qip-lifecycle/**`, `infrastructure/**`, or
`manifest_wiring.rs` — a concurrent lane owns §12.3 in those crates. Nothing
here adds a CRAWL stage (link-following, domain expansion — §7.4's own
remaining gap, separately scoped) or a network client; candidates still arrive
as a caller-supplied `Vec<SourceCandidate>`.

---

## Context

Three rows in `docs/DELIVERY-STATUS.md` were `ABSENT` and, by the blueprint's
own text, form one dependency chain rather than three independent gaps:

- **§7.6.1 (Source Categories).** The blueprint names eight categories —
  regulatory/legal, government/trade, corporate self-disclosure,
  physical/geospatial, community/technical, academic, marketplace, resolution
  sources — and §7.4's own Classify stage asks the question that assigns one
  ("is this news, filings, data, discussion, a marketplace, a leak forum?").
  Neither existed as code: `grep -rn 'pub enum.*Category\|SourceCategory'
  --include=*.rs backend/crates/services/qip-data-finder/src` returned
  nothing.
- **§22.3 (Data References).** No `DataReference` type and, specifically, no
  content hash — the field the section's own table calls "the single most
  important one, because it is what flags a backtest whose source revised its
  history." `qip-data-finder`'s existing `RegisteredSource` says a source
  *may* be used; nothing said what was actually *fetched*, from where, or in
  what shape.
- **§22.4 (Fetch-on-Demand for Research).** No campaign, no TTL cache, no
  hash verification on re-fetch, no manifest, and in particular no
  concentration-risk check: `grep -rn 'two registered\|second source
  \|concentration risk' backend/crates/services/qip-data-finder/src` returned
  nothing, so a universe backed by a single source was never held back.

§22.3 and §22.4 genuinely depend on §7.6.1: a data reference has to say what
*kind* of source produced it, and a fetch campaign reasons about
concentration and licensing per category. They are built in that order below,
and §22.4 is not built past what §22.3 supports.

## Decision

Three additions to `qip-data-finder`, each a new module following the crate's
existing one-topic-per-file convention (`tier.rs`, `coverage.rs`, `quality.rs`,
...), wired into the existing lifecycle rather than left as unused types.

### 1. `category.rs` — `SourceCategory`, `ContentSignal`, and a real refusal

`SourceCategory` is the eight-variant enum the blueprint tables. It is
classified from a new `ContentSignal` — a claim, declared on a
`SourceCandidate` via the new optional `SourceCandidate::with_content_signal`,
about what a location actually is (a regulatory filing, a customs record, a
specialist forum, ...). This crate reads no page and runs no NLP (§7.4's
Sample and Classify-by-content stages remain unbuilt, as the acceptance test
strategy names elsewhere), so the signal is exactly the kind of declared,
unverified claim `SourceCandidate::declared_coverage` and
`declared_licensing` already are — the same shape of pre-probe evidence
`tier::TierEvidence::from_candidate` already builds, extended to a second
question.

`SourceCategory::classify` refuses rather than guesses in two cases:

- **No signal declared** (`None`) — a candidate nobody has said anything
  about beyond its endpoint is unclassified, not defaulted to a category.
- **A signal declared that fits none of the eight** — `GeneralNews`,
  `UnspecialisedDiscussion`, and `LeakForum` are the three shapes §7.4's own
  question names ("is this news... discussion... a leak forum?") that the
  eight-category table does not admit. A general news wire is a surface-web
  feed the ordinary §7.3 pipeline already ingests; unspecialised discussion is
  a forum before anyone has read enough of it to say what it is *about*; a
  leak forum is excluded again, independently, at §7.5's hard line. Mapping
  any of the three onto the nearest of the eight would be exactly the
  force-fit `.claude/rules/00-enterprise-governance.md`'s "refuse rather than
  guess" forbids.

Wired into `DataFinder::assess_one`: classification runs at the existing
`LifecycleStage::Classify` step, its finding (or refusal) is recorded in the
decision's `Reasoning`, and a successful classification is carried onto
`RegisteredSource` as a new `Option<SourceCategory>` field — populated at
registration, not computed separately and left unused. A category is treated
as metadata a later `DataReference` will require, not a legality gate: an
unclassified source still registers if legality and score permit it,
consistent with the section's own framing of categories as a way to reason
about a source, not a new admission control.

### 2. `reference.rs` — `DataReference`, distinct from `RegisteredSource`

`DataReference` records the source (by id, requiring the `RegisteredSource`'s
already-recorded `SourceCategory`), a locator, the symbols and `DataPeriod`
(start/end `Timestamp`, refusing an inverted span) covered, the `SourceSchema`
the data was in, a SHA-256 content hash, `retrieved_at`, a `Decimal`
`cost_estimate`, and an `f64` `availability` in `[0, 1]`.

**The content hash reuses `qip_core::sha256_hex` — the exact mechanism
`qip_financial::manifest::SourceManifest` already uses for §7.2's own
content-hashed manifest** ("a manifest pointing at the original, with a
content hash"). No second hashing scheme was written. `DataReference::verify`
re-hashes a re-fetch and returns a `RevisionCheck` (`Unchanged` or `Revised {
was, now }`) rather than a bare `bool`, so a caller can report what a source
used to say without having kept the old hash somewhere else.

`DataReference::of` refuses:

- a `RegisteredSource` with no recorded category (§7.6.1 must have run);
- an empty locator or empty symbol set — the same "a reference to nothing is
  not a reference" argument `SourceManifest::of` already makes for its own
  locator, extended to what a reference *identifies*;
- empty bytes — `SourceManifest::of`'s own refusal, verbatim reasoning: the
  SHA-256 of nothing is the hash of every extent that never arrived;
- a negative cost estimate or an out-of-range availability.

### 3. `campaign.rs` — a bounded, TTL-scoped fetch campaign and the
concentration-risk check

`CacheBound` states a mandatory positive TTL and a mandatory positive entry
ceiling — never an unbounded cache, per
`.claude/rules/domains/data-and-streaming.md`. `ResearchCache` is a
`BTreeMap`-backed cache of fetched extracts keyed on locator; `insert` evicts
expired entries first and then refuses growth past the bound rather than
exceeding it. `FetchCampaign` is a named campaign (refusing a blank name)
holding one `ResearchCache` and producing a `CampaignManifest`: `fetch`
verifies a locator's bytes against whatever this campaign already cached for
it, flags every manifest entry recorded so far for that locator when the hash
disagrees, and records the new entry regardless — a revision is a fact the
manifest must carry, not an error that halts the run. `FetchCampaign::close`
drops the cache and returns only the manifest, which is exactly the
blueprint's own arrow: "the cache expires and is deleted... what persists:
the manifest and the results."

`assess_concentration` closes the specific gap the task named: given the
distinct, caller-filtered set of independently viable source ids backing a
data class, it returns `ConcentrationVerdict::Sufficient` at or above
`ConcentrationVerdict::MINIMUM_VIABLE_SOURCES = 2` and `HeldBack` below it —
§22.3's own number, named as a constant rather than inlined as a bare `2`.

## Which of §22.4's five mitigations this closes — read exactly, not rounded up

| Mitigation (§22.4's table) | Status | Where |
|---|---|---|
| Source revises history after use | **Built** | `DataReference::verify` + `CampaignManifest::flag_revision`, exercised by `FetchCampaign::fetch` |
| Research is slower than a local copy | **Built** | `CacheBound` + `ResearchCache`, scoped to one `FetchCampaign` |
| Regulatory demand for data not retained | **Built** | `CampaignManifest`, survives `FetchCampaign::close` |
| Vendor withdraws historical access | **Partial** | The two-registered-sources half is `assess_concentration`. The row's second sentence — "bar-level fallback retained for three years on traded instruments" — is §22.1's retention taxonomy (`RetentionClass`, a fallback OHLCV series), which `docs/DELIVERY-STATUS.md`'s own §22.1 row records as not existing. Building it here would be re-scoping this change onto a different section's absent foundation. |
| Sketch or reservoir error affects a model | **Not built** | §22.2's row records that no sketch (t-digest, reservoir, count-min, ...) exists anywhere in this codebase. A mitigation bounding a sketch's error has nothing to bound. |

Three of five built in full, one partial, one not started. §22.4 is scored
`PARTIAL` in `docs/DELIVERY-STATUS.md`, not `REACHED` — the task's own
instruction against rounding up a five-part mitigation to a pass on three and
a half parts.

## What was checked before committing to it

- **No new dependency.** Every new module uses `serde`, `serde_json`, and
  existing `qip-core`/`qip-contracts`/`qip-financial`/`qip-events` primitives
  already in `qip-data-finder`'s `Cargo.toml`.
  `./scripts/check-dependencies.sh` reports `11 third-party package(s), all
  permitted` after this change (unchanged).
- **The licensing gate is not bypassed.** `RegisteredSource` has a
  `pub(crate)` constructor reachable only from
  `DataFinder::assess_one`, which builds it only after
  `RegistrationDecision::registered` has refused to run unless
  `LegalAssessment::overall().is_permitted()` — the licensing question
  answered through `LicensingPosture::legality_for`. `DataReference::of`
  takes `&RegisteredSource`, so there is no constructor path from an
  unclassified, unlicensed, or unregistered candidate to a `DataReference` or
  into a `FetchCampaign`. `grep -n 'RegisteredSource::new' backend/crates/services/qip-data-finder/src/*.rs`
  finds exactly one call site, in `finder.rs`, after the gate.
- **No I/O added.** `campaign.rs` and `reference.rs` take bytes the caller
  already read, exactly as `SourceManifest::of` does; neither module opens a
  socket or assumes a transport.
- **Bounded retention.** `CacheBound` refuses a zero TTL and a zero entry
  ceiling at construction, and `ResearchCache::insert` refuses to exceed its
  bound rather than growing past it silently.
- **`BTreeMap`/`BTreeSet` used wherever iteration order reaches output** —
  `ResearchCache`'s entries, `assess_concentration`'s distinct-source set,
  `DataReference::symbols` — per `.claude/rules/domains/core-rust.md`.
- **Mutation-verified**, per `.claude/rules/architecture/01-testing-strategy.md`:
  every new test's assertion was confirmed to fail for the stated reason
  under the named mutation, then the code was restored byte-for-byte and the
  test re-confirmed passing. See the commit message and the task's evidence
  report for the specific mutation and result per test.

## Alternatives considered and rejected

**Classify categories by reading the sampled payload's body or media type**
(an automatic classifier). Rejected: nothing in this crate parses natural
language or reads a page's content for meaning (§7.4's Sample stage is itself
unbuilt), and inferring a category from a JSON shape or a content-type header
would be a guess wearing a classification's clothes — exactly what "refuse
rather than guess" forbids. A declared claim, refused when it does not fit,
is the honest version of this stage until Sample exists.

**Make `ContentSignal` a required constructor argument on `SourceCandidate`.**
Rejected: `SourceCandidate::new` is called from `qip-kernel`'s tests,
`qip-deepbrain`'s `discovery.rs` and `main.rs`, and `qip-acceptance`'s
`e2e.rs` — several of them in crates this change may not touch. An optional
builder method (`with_content_signal`, defaulting to `None` via
`#[serde(default)]`) closes the same gap without forcing an edit onto
`qip-kernel`.

**Store the fetched bytes inside `DataReference` itself**, rather than
separately in `ResearchCache`. Rejected: a reference is meant to survive after
a campaign closes and its cache is deleted (the blueprint's own arrow); a
`DataReference` carrying the bytes would make "the cache is deleted" a lie the
type contradicts every time a `DataReference` is retained in a manifest.

**A single hashing scheme reused by inlining `sha256_hex` calls without
citing `SourceManifest`.** Rejected in favour of the explicit reuse called out
above: `.claude/rules/architecture/00-boundaries.md`'s "no second source of
truth for a fact the event log already holds" reads the same way for a
hashing *mechanism* as for a fact — two independently-written SHA-256 call
sites for "these bytes are what we saw" is not two sources of truth today, but
it is the shape one becomes the first time someone tunes one and not the
other.

**Fold `assess_concentration` into `DataFinder` as a method.** Rejected: what
counts as "viable" and what a "data class" is are caller judgements this
crate has no concept of (an asset class, an instrument, a symbol group);
`DataFinder` owns per-source lifecycle decisions, not a caller's own
partitioning of its universe. A free function taking the caller's own set is
the honest boundary.

## What it costs

**A category, once declared, is never re-verified against the source's actual
content** — the same limitation `declared_coverage` and `declared_licensing`
already carry, extended to a third field. Nothing in this crate samples a
page to check a claim; §7.4's Sample stage would be the place that changed,
and it remains unbuilt, named as such in `docs/DELIVERY-STATUS.md`.

**§22.4 is not fully closed.** The three-year bar-level fallback series and
any sketch-error bound remain open, tracked against §22.1 and §22.2
respectively rather than duplicated here.

## What would make this wrong

- **A future change builds §22.1's retention taxonomy or §22.2's sketches and
  this record is not revisited** — the day either lands, §22.4's `PARTIAL`
  scoring here should be re-checked against the fuller mitigation set rather
  than left citing an absence that no longer holds.
- **A caller constructs a `DataReference` or a `FetchCampaign` entry from a
  `Source` that never went through `DataFinder::assess`** — there is no such
  constructor today; if one is added, it must go through the same legality
  and category gates `assess_one` already runs, not around them.
- **`ConcentrationVerdict::MINIMUM_VIABLE_SOURCES` is changed to fit a
  demonstration** rather than left at the blueprint's own stated number, two.
