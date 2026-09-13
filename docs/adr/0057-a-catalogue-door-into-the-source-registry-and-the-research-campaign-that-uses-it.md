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

**Amended in place a second time, 2026-09-12**, after the repair commits
(`93c4c7b..8a19737`) were themselves independently re-reviewed and found
one high finding, one blocking, three medium and a set of lower ones. Every
one is fixed in the commits that follow `8a19737`, and this record is
corrected where those commits made it false. The two that change what this
record decides: **a replay's bytes are never a vendor** — the replay door is
its own `SourceOrigin`, `ReplayedAdmitted`, and the concentration rule does
not count it, because the earlier text let a hand-written file open rule
31's hold on zero vendor bytes — and **the deep brain has a connector arm**,
so the only way its ledger holds a vendor's standing is a fetch this process
made from the vendor. Sections 3, 5 and 7, the mitigation table, the checks
and the costs are the amended text; the earlier claims they replace are
named where they stood.

**Amended in place a third time, 2026-09-12**, after the second round of
repairs (`8a19737..2c954cd`) was re-reviewed by a fresh security-engineer
and a fresh code-reviewer: one high finding, two medium, three should-fix
and a set of lower ones, every one fixed in the commits that follow
`2c954cd`. Nothing this record *decides* changes; four things it *claimed*
were false or unpinned and are corrected below. The retained-chain check
failed on any log with an interior eviction, so `Platform::new` refused to
restart over its own honest log (§3, now stated with what the chain
actually proves: unkeyed SHA-256, ADR 0043). A stream's provenance was
inferred from the configuration and pinned by nothing; it is now the
adapter's own answer (§5). The connector arm accepted any plaintext
`http://` host in the process, with the loopback rule held by Terraform
alone; it is refused at three seams now (§7). And the test this record
cited as proof that replays never lift the hold iterated an empty history
(§7, corrected to what holds it). The checks and costs carry the smaller
findings: a poll's ledger and checkpoint as one adopt-on-success write,
one process per log file, the sketch's rows held to its total, the
reference's locator held to its door and its hash to its shape, and each
source observed as soon as its own poll succeeds.

**Amended in place a fourth time, 2026-09-12**, after the third round of
repairs (`2c954cd..672563f`) was re-reviewed by a fresh security-engineer
and a fresh code-reviewer: no blocking or high finding; one medium, two
low and a set of wording items, every one fixed in the commits that follow
`672563f`. Nothing this record *decides* changes; three things it
*claimed* in its costs and checks were wrong and are corrected below. The
log lock's cost said a held log could not be inspected because the type
had no read-only open; the writer's open was also the only open, so
`qip replay` failed on a read-only mount and relabelled the lock refusal
as a corrupt file, and it now reads through `EventLog::inspect` (§costs).
The loopback check was a second URL parser that read a userinfo trick as
loopback and admitted `localhost` where Terraform admits only the
literal; it parses with the transport's parser and admits `127.0.0.1`
alone, on the API's parser as well as the brains' (§7, checks). And the
journal's "one poll ahead, heals itself" cost was true only inside the
process that failed; the ledger and the checkpoint are one value under
one key now (§costs). The remaining items are wording, one assertion, and
the deep brain's error exit releasing its connector sessions.

**Amended in place a fifth time, 2026-09-12**, after the fourth round of
repairs (`672563f..4cb4983`) was re-reviewed by a fresh security-engineer
and a fresh code-reviewer: no blocking or medium finding; eight low and
should-fix items, every one fixed in the commits that follow `4cb4983`.
Nothing this record *decides* changes; two things it *claimed* were
narrower than their sentences and are corrected below. "One parser, one
spelling" was true of the three connector-pair gates and not of every
in-process base URL: the deep brain's hosted language-model listener —
the one address that carries a bearer token — was still two string
prefixes admitting `localhost` and a userinfo trick, and the fast brain's
market-data vendor refused `https` alone. The gate now lives beside the
parser, `qip_transport::http::require_loopback_egress`, holds every
credential-bearing address (six seams), requires the explicit port
Terraform has always required, and redacts userinfo from every refusal it
writes — because the refusal itself was echoing the credential it refused
(§7, checks). The one in-process URL that does not go through it,
`QIP_OPENOBSERVE_URL`, is named as the exception with its reason (ADR
0032 puts that collector on a private VPC address, not on loopback), so
the sentence now covers exactly the gates it claims. The inspection's
cost said an append to an inspected log "reaches memory and never the
file", and that was a defect described as a shape: the log refuses it by
name now (§costs). The billing test's mutation note described a mutation
its own store could not admit; the note now records the one that fired.
The deep brain's flush exit and its open loop skipped the release the
error exit had gained. And this record's claim that one inspection test
was "run unprivileged" is replaced by what the checkout can show.

**Amended in place a sixth time, 2026-09-12**, after the fifth round's
commits (`4cb4983..717cd81`) were pushed to origin **before** review and a
fresh security-engineer then found one blocking defect in them: the fifth
amendment's own redaction was incomplete. State this plainly, because it is
the reason this round exists — a real credential-leak defect shipped to
origin. `redact_userinfo` found the userinfo it was written to mask by first
requiring `"://"`; a base URL with the scheme dropped by a configuration
mistake — `QIP_LANGUAGE_MODEL_BASE_URL=svc:TOKEN@127.0.0.1:9106`, the `http://`
missing — has no `"://"`, so the function returned it unchanged, and both of
`require_loopback_egress`'s own `shown` and `Url::parse`'s `invalid` closure
(which calls `redact_userinfo` a second time on the same raw string once the
parse fails for having no scheme) printed `TOKEN` in the clear, twice, to the
fatal start-up error every one of the six egress call sites wraps. The fifth
amendment's claim that the gate "redacts userinfo from every refusal it
writes" was true of every credential-bearing string that carried a scheme and
false of the one shape a missing `http://` produces — the exact operator
mistake the gate exists to catch. `redact_userinfo` no longer requires a
scheme to find the authority: it treats everything up to the first `/`, `?`
or `#` as the candidate authority whether or not `"://"` was found, and
redacts an `@` inside it either way (`qip-transport/src/http.rs`). Fixed in
the commit that opens this round, ahead of the two should-fix items below, and
pinned by `a_scheme_less_credential_bearing_egress_address_is_still_redacted`
in `qip-transport/tests/http_client.rs`, which drives the exact scenario
above through `require_loopback_egress` and checks both `Display` and
`Debug` of the resulting error for the token. The existing
`redact_userinfo("no scheme@here")` assertion pinned the old, wrong
behaviour (pass-through); it now asserts `"…@here"`, with a note that the old
expectation was the defect, not a premise worth keeping.

Two should-fix items from the same review, neither a leak: `qip-deepbrain`'s
`with_release` composed the arm's and the evolution connectors' shutdowns
with `.and()`, which keeps only the first `Err` — an operator reading a
double-failure exit sees one dead session named and not two. It now folds
both messages, mirroring the `relabel` construction already used in this
file, so a double release failure names both. And the admission-check
failure arm inside `ConnectorArm::open` (`qip-deepbrain/src/connectors.rs`)
— the point where a feed has already opened its socket but the licensing
decision then refuses it — dropped the feed via its plain `Drop` rather than
calling `shutdown()`, and the identical shape exists, also untouched, in
`qip-api/src/feed.rs` and `qip-fastbrain/src/feed.rs`. The prior round's
commit message claimed "every exit that leaves before the clean one" releases
its connectors; that was true only of the two exits inside each root's
`main.rs`, not of this third exit inside the constructor itself, which none
of those roots ever sees. All three constructors now call `shutdown()` on
that one failure arm before returning the admission error, narrow-scoped to
exactly that arm, so the claim is true rather than corrected to a narrower
one. `ConnectorArm::over_transport_admitted_by`, the fourth site sharing the
identical shape, is fixed the same way; it is also the only one of the four
a unit test can drive without a real socket, and it is what
`an_admission_refusal_after_the_feed_opens_still_releases_it` exercises — a
manifest with no §7.6.1 category, refused by `AdmittedSource::from_decision`
after a real Coinbase manifest's connector has been wrapped in a spy that
records whether `shutdown` ran. The other three open a real socket through a
shipped connector, whose manifest declares a consistent class and category
by construction, so there is no test-only way to make the admission check
fail after one of them has opened without fabricating a defect in a shipped
connector's own manifest; those three are proven by code inspection and by
the unchanged clippy and fmt gates rather than by an executed failure case,
and that is stated plainly rather than left to be inferred from a passing
test suite that never reaches the arm. This is inert today, as it was
before — every shipped `SourceConnector::shutdown` is a no-op default — and
the fix costs nothing disproportionate, so the code fix was preferred over
amending the claim alone.

**Amended in place a seventh time, 2026-09-13**, after a fresh adversarial
security review of the sixth amendment's own commit (`06d2718`, landed
inside `41f25ad`) found that fix was *also* incomplete — the third
consecutive round in which a "complete" redaction fix left a real leak.
State this plainly, as the sixth amendment stated its own predecessor's
failure plainly, because the pattern is the finding: rounds one and two
each repaired the one input that had been reproduced against them and left
the underlying method — search the string for a marker, trust whatever
precedes it — in place for the next adversarial input to exploit.
`redact_userinfo` and `Url::parse` both located the scheme boundary with
`raw.split_once("://")`, and `split_once` finds the *first* occurrence of
`"://"` **anywhere in the string**, not the one at its start. A
scheme-less credential whose path or query contains the ordinary substring
`"://"` — any `?redirect=`, `?callback=` or `?fallback=` parameter naming
another URL, not a contrived shape — supplies exactly such a later
occurrence:
`redact_userinfo("svc:TOKEN@127.0.0.1:9106/callback?redirect=http://evil.example/x")`
came back **unchanged, `TOKEN` in the clear**, because `split_once` matched
the query's `"://"` instead of finding no scheme at all, and everything up
to that match — the real credential included — was read as "the scheme",
which the authority search then had no reason to look inside for an `@`.
The sixth amendment's claim that `redact_userinfo` "treats everything up
to the first `/`, `?` or `#` as the candidate authority whether or not
`"://"` was found" was true only when the string's *one* `"://"`, if any,
was the leading one; it did not hold for a string whose only `"://"` was
buried in a query parameter, which is an unremarkable shape and not an
edge case.

A second, structurally separate finding rode the identical defect:
`HttpError::UnsupportedScheme { scheme }` stores and prints whatever
`Url::parse` computed as "the scheme", with no call to `redact_userinfo`
at all, because a value that can only ever be a clean RFC 3986 scheme
token needs none. With the unbounded search, the same adversarial input
made `Url::parse` compute the entire credential-bearing prefix as "the
scheme" — `Url::parse` on the input above returned
`UnsupportedScheme { scheme: "svc:token@127.0.0.1:9106/callback?redirect=http" }`
— and that variant's `Display` printed it outright. Fixing
`redact_userinfo` alone, as the sixth amendment did, would have left this
arm printing the identical credential by a different path; the two are
named as separate findings because they are two separate call sites that
happened to share one root cause, not one finding with two symptoms.

Both are fixed by replacing the unbounded search with `split_scheme`, one
function both `Url::parse` and `redact_userinfo` now call: it checks
whether `raw` **starts with** a token matching the RFC 3986 scheme grammar
(`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`) immediately followed by
`"://"`, rather than searching for `"://"` anywhere in the string. This is
the structural distinction the first two rounds missed: RFC 3986 makes a
scheme the literal prefix of a URI, never something a parser is entitled
to find by scanning the tail, so anchoring the check to position zero is
not a tighter version of the same method — it is the correct grammar in
place of an approximation that happened to work on every input reproduced
against it so far. Because `UnsupportedScheme.scheme` can now only ever be
produced by `split_scheme`'s bounded scan, it structurally cannot contain
`@`, `:` or `/`; this is reinforced with a `debug_assert` at the
variant's one construction site and proven with adversarial inputs
designed to try to break it
(`unsupported_scheme_never_carries_unbounded_content`,
`qip-transport/tests/http_client.rs`). The full fourteen-case matrix the
review specified is table-driven in
`redact_userinfo_handles_the_full_adversarial_matrix` and two dedicated
`Url::parse` tests, and the two cases that matter most are
mutation-verified: reverting `redact_userinfo` to the sixth amendment's
`split_once("://")` shape reproduces `TOKEN` unredacted on the exact input
above, and reverting `Url::parse`'s detection (with the new `debug_assert`
also removed, since it would otherwise catch a bare regression before the
test's own assertions ran) reproduces the `UnsupportedScheme` finding
byte-for-byte. Both restored and re-verified passing.

Why this should not recur a fourth time, stated as the reason rather than
asserted as a hope: the first two rounds each had one detector doing its
own ad hoc string search, so a fix to one detector's search left the other
— or, this round, a second call site reachable through the same search —
unrepaired. There is now exactly one function that decides where a scheme
ends, it is anchored to grammar rather than to a marker's position in the
string, and both call sites (three, counting `UnsupportedScheme`'s
implicit reliance on it) are proven, by the field's construction, to be
downstream of it rather than each free to disagree about what a scheme is.

Also addressed, from the same review, three should-fix items left open by
the sixth amendment's own commit: `let _ = feed.shutdown(at);` at all four
connector-release sites (`qip-deepbrain::connectors`'s two constructors,
`qip-fastbrain::feed`'s and `qip-api::feed`'s) discarded a real shutdown
failure rather than folding it into the returned error, now fixed with
`qip_core::error::Error::and_release` — the `with_release`/`fold_releases`
pattern this file already described, generalised into `qip-core` so three
crates that had no such machinery share one implementation rather than
each writing their own; `relabel`'s doc comment claimed "two callers" when
`fold_releases` had been a third since it was added, corrected; and the
scheme-less-path-`@` permutation flagged as untested turned out **not** to
be subsumed by the existing scheme-plus-query-`@` case (which branch of
`redact_userinfo` runs depends on whether a scheme was found at all), so it
is now its own row in the test matrix rather than an unverified claim.

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

The second review found the rebuild believed what it read. `EventLog::open`
parses the file and recomputes no hash, `DataReference` and `RevisionRecord`
derived `Deserialize` with no gate in front of it, and `resume_references`
restored every frame verbatim — so a frame edited on disk, or forged into a
shape `build_hashed` refuses, became the ledger's latest reference. Now both
types deserialise through their constructors (`#[serde(try_from)]`, the
`ErrorBound` pattern), the retained chain is verified before any frame is
restored (`EventLog::verify_retained_chain`) and a broken link refuses the
resume by sequence, the fabric journal's posture.

The third review found that check wrong in the other direction. The log
evicts its oldest evictable record *wherever it sits*, so once a permanent
record — a closed campaign, a revision — is older than every evictable
one, the next eviction opens a gap in the interior of the retained span;
the check re-anchored only the head and then held every record to its
retained predecessor, so it failed at the first record after such a gap,
and a file-backed deep brain refused every restart over its own honest log
naming tampering. Now every record's own hash is recomputed, a link is
held only between consecutive sequences, and across a gap the record is
re-anchored on its claimed predecessor exactly as the head is — the same
trust level, no new assumption. A gap can be read as an eviction because
the file cannot have one: `EventLog::open_with_capacity` refuses a file
whose sequences are not contiguous from one, before any hash is looked at,
so a line removed from the file is refused at load and never reaches the
retained check as a gap. What the chain proves is stated exactly, because
the refusal's wording overclaimed it: it is **unkeyed SHA-256** (ADR 0043's
anchoring gap), so a broken link means the bytes read are not the bytes
written — a payload edited with its hash left as written — and not that a
writer who could recompute every hash after it has been kept out. A
catalogue-admitted reference counts as backing only while this process
holds the source's admission (`Platform::sources_backing` intersects the
ledger with the admission table; `Platform::withdraw_source` is what a
lapsed gate calls), because a reference restored from the log is a fact
about what was fetched then, not a vendor behind the subject now. And
`SourceRevisionDetected` carries the revising reference beside the finding,
at schema version two: the reference's own record is Sense-group and
evictable while the revision is permanent, and a log that had evicted the
former restored a ledger that held no latest reference to the extent, so
the next revision of it was missed. A version-one frame is refused at
resume rather than half-read.

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
the platform before the door is resolved — and, since the second amendment,
withdraws it from the platform on the round the gate stops granting. The
first amendment said this was "the one way a deep brain without a live
vendor call researches through the catalogue door, and what lets a second
vendor back a subject". The second review showed that to be the defect it
sounds like: a hand-written bars file headed with the ECB connector, then
restarted under the Coinbase header, was two vendors on the ledger, and rule
31's hold opened on zero vendor bytes with the ECB's licence attributed to
fabricated bars on the permanent log. So a replay under an admission is now
referenced through the **replayed door**, `SourceOrigin::ReplayedAdmitted`,
with a `replay://` locator so the ledger says on its face that the bytes
were read from a file, and `is_independent_vendor` is false for it: the
licence is the gate's answer, the bytes are the file's author's word, and
only a fetch this platform made is evidence the vendor served it. The
adapter also holds the file to the source it names — every record must be
of a topic the connector ships, so a bars file headed with a ticker
connector is refused at open — and refuses a second header or one after the
first record. No shipped connector ships a bar, so no file honestly recorded
from one carries a window the desk can fit; the door exists for the day a
bar-emitting connector is admitted, and it will never count as a vendor.
Campaign ids carry the log's last sequence beside the cycle, because the
cycle count restarts at one in every process and after any restart every
manifest was suppressed as a duplicate of the previous run's while the
round line said "manifest journaled"; `Platform::journal_campaign` now
returns whether it wrote and `assemble` refuses a `false` for an id it has
just minted. The summary carries no `journaled` flag: a summary exists only
past that refusal, so the bool the second amendment added had one value,
and `describe` states the fact instead.

Which stream is a replay is, since the third amendment, the adapter's own
answer — `DataAdapter::provenance`, `Live` by default and `Replayed` for
the two adapters that read a file, `ReplayAdapter` and `TapeFeed` — and
not an inference. The engine read `Replayed` off the presence of a standing
admission, a fact about the configuration rather than the bytes: an
undeclared replay read as live, and nothing at the engine level pinned it,
so a mutation to "always live" was caught by no test. `StreamProvenance`
lives in `qip-market-ingestion` beside the trait and the campaign
re-exports it.

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

### 7. The deep brain's connector arm — the only way its ledger holds a vendor's standing

With replayed bytes no longer a vendor, the deep brain had no way to hold a
vendor's standing on its ledger, and the hold could open in no
configuration. `qip_deepbrain::connectors::ConnectorArm` is the fast
brain's `Feed::Connector` on the research node: one or more catalogued
connectors, each opened through the licensing gate before any socket, each
with its stream's durable record opened before the first poll, each
re-asked at the instant of every poll, and each fetch digested where its
bytes existed and referenced on the platform's ledger between the poll and
the checkpoint commit. The arm is additive rather than a replacement — the
learning desk fits on bars and no shipped connector emits one, so the own
stream keeps feeding the desk while the arms feed the platform's
observations and its ledger, which is what the concentration rule counts.
A gate that stops granting withdraws its source and refuses the poll, which
stops the node as it stops the fast brain; the replay path keeps its
refused-round posture because a replay is a file this repository owns and a
vendor is not.

ADR 0024 is why this is the right binary: the deep brain carries the egress
sidecar and the fast brain deliberately does not (ADR 0008), so the arm the
fast brain has always had is one it cannot use — `manifest_wiring.rs`'s
credential-mount rule says so — and this is the first on a workload with an
outbound path. The root reads the variables the other roots read,
`QIP_CONNECTOR_SOURCE` (a comma-separated list here, since a process fed one
connector can never hold two) and `QIP_CONNECTOR_BASE_URL`; both or
neither, never the vendor's own address, a repeated id refused, a tape
beside a connector refused as a contradiction of clocks. The Terraform half
is a root variable `deepbrain_connector`, null in every environment with
the reason beside it, rendered into the deep brain's catalogue entry in a
conditional arm so `manifest_wiring.rs`'s allowlist gains nothing. Proven
end to end over scripted transports through the real gate and runtime:
two live admitted connectors over one subject lift the hold and one does
not (`two_live_admitted_connectors_over_one_subject_lift_the_hold_and_one_does_not`).

That replays never do was, until the third amendment, cited to
`replays_under_two_vendors_admissions_back_no_vendor_and_never_lift_the_hold`,
whose closing check iterated the engine's bar history — and a tick replay
has none, so it ran zero times. What actually held the door was the
campaign-level
`a_replay_under_admission_is_referenced_through_the_replayed_door_and_backs_no_vendor`
and, at the ledger, `a_replay_under_a_connectors_admission_is_not_a_vendor`.
The engine test now asserts its premise and asks `sources_backing`
directly over a subject given bars by hand, with a second vendor's
reference on the ledger through the replayed door: both read
`ReplayedAdmitted`, the verdict counts zero vendors, and two mutations —
`is_independent_vendor` widened, the `Replayed` arm deleted — each fail it.

Three more corrections to this section from the same review. The arm
accepted any plaintext `http://` host in the process: `connector_feed`
refused only `https://`, and the loopback requirement lived in
`variables.tf`'s validation alone — on a binary with an egress sidecar,
and not the only one: the API has one too (ADR 0024), and the third
amendment's "the one binary with an egress path" was wrong about that.
`qip_market_ingestion::connector_feed::require_loopback_egress` — an
absolute `http://` URL, parsed by the transport's own parser, whose host
is the literal `127.0.0.1` and nothing else — was, as of the fourth
amendment, called at four seams: `ConnectorFeed::open`, the deep brain's
parser, the API's, and, for parity, the fast brain's (the fifth
amendment moves it and widens it; see below). The third amendment admitted `localhost`
beside the literal and parsed the address by hand; the fourth review
found the hand parser read `http://127.0.0.1:9105@evil.example/` as
loopback (the transport's refusal of userinfo was all that kept the
socket shut) and that `localhost` is a name the resolver answers rather
than a verified address, which is why Terraform never admitted it. Both
are corrected: one parser, one spelling. Terraform catches the committed
mistake, the process the unreviewed one. The fifth review found that
sentence true of the connector pair and not of the process: the deep
brain's hosted language-model listener, the one address carrying a
bearer token, was still gated by two string prefixes that admitted
`localhost` and read `http://127.0.0.1:9106@evil.example/` as loopback,
and the fast brain's market-data vendor refused `https` alone — that the
fast brain has no egress sidecar is a fact about the VPC, not a
guarantee the process holds. The gate is now
`qip_transport::http::require_loopback_egress`, beside the parser it
must agree with, and `qip-market-ingestion`'s copy is gone; it is called
at six seams — the connector pair in the API's, the deep brain's and the
fast brain's parsers, `ConnectorFeed::open`, the language-model listener
and the market-data vendor — and it requires an explicit port, which
Terraform's `startswith("http://127.0.0.1:")` always did and the process
until then did not. Every refusal it writes goes through
`redact_userinfo`, and so does `HttpError::InvalidUrl`, because the
refusal of `http://svc:TOKEN@127.0.0.1:9105` was itself printing `TOKEN`
on stderr at start-up. The one in-process URL outside it is
`QIP_OPENOBSERVE_URL`, on purpose: ADR 0032 decides that collector sits
on a private VPC address, not on loopback, so "one parser" holds for it
(it is `Url::parse`) and "one spelling" does not, by decision rather
than by omission. Multi-arm `sense`
collected every source's records into one batch and observed it at the
end, so an arm whose poll refused after an earlier arm had referenced,
journaled and committed its fetch left that batch neither delivered nor
re-fetchable — the loss `poll_referencing` closes inside one poll,
re-opened one seam up; each source is now observed the moment its own
poll succeeds, and nothing accumulates across the loop. And
`ConnectorArm::shutdown` had no caller; the root releases every arm's
session after the flush.

## Which of §22.4's five mitigations this closes — read exactly

| Mitigation (§22.4's table) | Status | Where |
|---|---|---|
| Source revises history after use | **Built, with a consequence** | `ReferenceLedger::assess` → `Platform::record_reference`: log record under its own topic, the closed campaigns that read the original named on the log, metric, `revision_covering` flag; `campaign::assemble` flags its own manifest only where it read the withdrawn bytes |
| Research is slower than a local copy | **Built, on the path** | `FetchCampaign` under `CampaignConfig::cache`; the window is read back from the cache |
| Regulatory demand for data not retained | **Built, on the log** | `ResearchCampaignClosed` on the log under its own permanently retained topic, and nowhere else |
| Vendor withdraws historical access | **Built; shut in every shipped deployment** | `assess_concentration` over vendor doors only, gating promotion in `EvolutionEngine::turn`; `FallbackSeries` fed from `observe`, drawn on by `assemble` and recorded on the manifest. The gate opens for two live admitted connectors over one subject, never for replays, and for nothing shipped — see the costs |
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
  `FallbackSeries` do not deserialise either; `ErrorBound`,
  `SketchedStatistic`, and since the second amendment `DataReference`,
  `RevisionRecord` and `CountMinSketch` deserialise through their
  constructors — the first two had a derive that the first amendment's
  claim overlooked, and the sketch's accepted a zero width. The chain over
  the log's retained span is verified before a frame is restored. Since
  the third amendment the wire gates hold more than shape: a
  `DataReference` is refused unless its `replay://` locator and its
  `ReplayedAdmitted` origin come together or not at all, and unless its
  hash is sixty-four lowercase hex digits — an uppercase copy of a digest
  would have read as a revision of the extent — and a `CountMinSketch` is
  refused unless every row sums to its declared total, which is the
  count its error bound is stated against. Each refusal has a test and
  each test was mutated.
- **Every credential-bearing base URL is loopback in the process, not
  only in Terraform, and the process and Terraform mean the same thing by
  it.** `qip_transport::http::require_loopback_egress` at
  `ConnectorFeed::open`, the connector pair in the API's and both brains'
  parsers, the deep brain's language-model listener and the fast brain's
  market-data vendor; the address is parsed by
  `qip_transport::http::Url::parse`, its host must be the literal
  `127.0.0.1`, and its port must be written. An `https` address, an
  RFC 1918 host, a vendor host, a `127.0.0.1.evil.example` host, both
  `localhost` spellings, `[::1]`, a port-less address, both userinfo
  spellings and a bare authority are each refused by a test; the third
  amendment said "both spellings of loopback admitted", and that admission
  is withdrawn; the fourth said this of the connector pair alone and
  called it every gate, and the two gates it did not cover are covered
  now. A refusal never echoes a credential: the gate's messages and
  `HttpError::InvalidUrl` carry the address with its userinfo replaced by
  `…@`, and a test holds the refusal of `http://svc:TOKEN@…` to not
  contain `TOKEN` at the parser, at the gate and at both brains' parsers.
  The exception is `QIP_OPENOBSERVE_URL`, which ADR 0032 places on a
  private VPC address by decision; it parses through the same `Url::parse`
  and is not held to loopback, and this record says so rather than
  letting "every" cover it.
- **One process per log file, and one write per poll.**
  `EventLog::open_with_capacity` takes an exclusive advisory lock
  (`std::fs::File::try_lock`, which is why the workspace's declared MSRV
  is 1.89) and refuses a log another handle holds, so two writers cannot
  mint one sequence — the uniqueness campaign ids claim across restarts
  was silently false across concurrent writers. `EventLog::inspect` is
  the reader's open: read-only, never creating, under the shared side of
  the same lock for the read and no longer, so `qip replay` checks a
  journal on a read-only mount and is refused a journal a node holds by
  the message naming the holder. `StreamJournal::record_and_commit`
  writes the ledger and the checkpoint as one value under one key in one
  `put`, so a failed write leaves the store, this process and the next
  process at the last poll that succeeded; the third amendment's "ledger,
  then checkpoint" left the durable ledger one poll ahead across a crash.
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
- **The deep brain reads the connector pair the other roots read**, and the
  Terraform half is in the same change: `deepbrain_connector`, null in
  every environment, rendered in a conditional arm, so `manifest_wiring.rs`'s
  allowlist gains nothing. The first amendment said no environment variable
  was added; the second adds none by name and gives one binary two it did
  not read before. The replay's `# recorded-from:` header is still a line
  in a file the existing variable already names.
- **The changed files touch no file under** `qip-risk-engine`,
  `qip-execution-engine`, `qip-capital`, `qip-edge`, `qip-brokers`,
  `qip-routing` or `qip-compliance`; under `infrastructure/` only the
  variable, its catalogue arm and the tfvars prose that explains the null.

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
shape provided until the second amendment. The first amendment said the
gate "can open without a live vendor call" for two admitted replays; that
was the defect, and it is withdrawn. The gate opens for two *live* admitted
connectors over one subject, proven end to end in
`two_live_admitted_connectors_over_one_subject_lift_the_hold_and_one_does_not`,
and replays never open it. In every shipped deployment it is shut:
`deepbrain_connector` is null everywhere, the proxy's bootstrap names only
the ECB host, and no two shipped connectors share a subject. That is the
rule working; a deployment that wants promotion needs a second vendor for
the subject and a process fed both, which is now a configuration a
deployment can state and which the rule exists to demand.

**The deep brain fails closed per subject at the door, and per process at a
connector arm.** A stream the door refuses — an undeclared replay, a replay
whose named source the catalogue refuses, a standing admission that has
lapsed — is a learning round with no campaign, on the round line and
counted, every round, for as long as the stream is refused; the node keeps
cycling and fits nothing, and a lapsed admission is withdrawn from the
platform on the round it lapsed. The first version stopped the process on
the first such round, which read as a crash and was the door working. A
connector *arm* whose gate stops granting takes the fast brain's posture
instead: the source is withdrawn and the poll's refusal stops the node,
because an empty batch and a vendor this platform is no longer licensed to
read must not be indistinguishable downstream. The earlier text said
"connector-fed" of a deep brain that had no connector; it has one now.

**A lapsed connector licence stops the research node.** The cost of the
posture above: a vendor whose terms expire mid-run takes the deep brain out
of rotation until an operator re-admits or unconfigures it, rather than
leaving it cycling on its own stream with the vendor's references quietly
gone from the count.

**A `SourceRevisionDetected` frame written before the second amendment is
refused at resume.** The body is schema version two and the revising
reference is required; a version-one log holding one cannot rebuild its
ledger. Nothing is deployed and no committed log holds one, so no
migration is written; a log that does needs archiving, not editing.

**A source a previous process admitted backs nothing until this one admits
it.** The ledger restores the reference; the count waits for the gate.

**A replay research window under a shipped connector's name cannot exist
today.** No shipped connector ships a bar, the adapter refuses a file whose
records the named connector never ships, and the desk fits on bars; the
replayed door is reachable only once a bar-emitting connector is admitted.

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

**A replay's `# recorded-from:` header is a claim by the file's author, and
the platform no longer extends it a vendor's standing.** It verifies that
the named source's licence exists and is granted, holds the file's records
to the topics that source ships, and references the bytes through a door
that counts for no vendor. The first amendment said the file's author was
"answerable" for the bytes being the vendor's; the second review showed
that answerability was the whole of the control, and it is replaced by
structure.

**The platform holds a second copy of the feed's admission.** Two records
of one decision, derived one from the other at a stated seam; if the seam
is bypassed, the platform refuses rather than guesses, which is the cost
paid in the API's feed test before the seam moved.

**The sketch on a one-subject campaign is exact**, and its bound is formal
rather than load-bearing today. What it earns its place with is the
refusal path and the memory bound a many-subject campaign would draw on.

**Eviction by insertion order** means a re-fetched extent ages out on the
ledger's schedule, not its own; the trade is stated in the ledger's doc.

**A log a running node holds cannot be opened by anything else, the CLI's
replay included — and the replay is told so.** The writer's lock is
exclusive; `EventLog::inspect` takes the shared side of it and is refused
while a node holds the file, with the message naming the holder, because
a reader of a file mid-append reads a partial record and "corrupt at line
n" would be the wrong diagnosis. Copy the file and inspect the copy. The
third amendment said the type had no read-only open at all, and the
consequence it did not state was that `qip replay` used the writer's
open: on a read-only mount — where an archive is kept on purpose — the
append open failed and was reported as "not an event log this platform
wrote", the lock refusal was relabelled the same way, and a mistyped path
created an empty file. The inspection is read-only, creates nothing and
releases its lock with the read; the CLI passes the lock refusal through
unwrapped. And an inspected log refuses `append` by name, telling the
caller which open it came through and which to use to resume the file:
the fourth amendment's inspection carried a file's records and no path,
so an append "reached memory and never the file" — a chain the file did
not hold, with nothing to say so, which its own test asserted as the
intended shape until the fifth review. The lock is advisory — it holds against everything that opens
the file through `EventLog` and against nothing that writes the bytes
another way — and it is proven so on Unix only: the workspace is built
and deployed on Linux, there is no Windows CI, and what `std` maps the
call to elsewhere has not been exercised.

**The journal is one value under one key, and a store holding the two
old keys is refused.** The third amendment made the ledger and the
checkpoint one step over two keys, ledger first, and stated that a
ledger write succeeding before a checkpoint write failed left the
durable ledger one poll ahead "until the next successful write". That
held only inside the process that failed: the failure propagates out of
`poll_referencing`, the deep brain exits on it, and the restart loaded
the ledger already counting the poll, resumed from the older checkpoint
and billed the re-fetch again — one high for the life of the stream.
One `put` of one value is atomic on every store's own terms (a map
insert; an atomic rename with `fsync`; a `SET`), and no store here offers
a multi-key transaction. The cost is the layout change: nothing is
deployed, so no migration is written, and a store still holding
`…/ledger` or `…/checkpoint` is refused at open by name with the remedy
rather than read as a fresh stream. `StreamJournal::record`, the first
half of the old double bill, is gone.

**The `Discovered` door has no live gate at `sources_backing`.** The
discovery path's registration reaches no real bytes in production
(`NetworkProbe` refuses every call), so nothing puts one on the ledger;
the day one does, it needs the same intersection the catalogue door has.

**The declared MSRV is 1.89.** `File::try_lock` stabilised there; the
pinned toolchain is well past it, and the lints the raise woke were
applied rather than silenced.

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
- **A door that counts a replay as a vendor again.** `SourceOrigin::
  is_independent_vendor` must stay false for `ReplayedAdmitted`, `assemble`
  must resolve a replayed stream to that door, and the adapter must keep
  holding a headed file to its source's topics. Any one of the three
  relaxed re-opens the two-header scenario the second review found.
- **A catalogue-admitted reference counted without a live admission.**
  `Platform::sources_backing` intersects with the admission table; a caller
  reading `ReferenceLedger::sources_backing` directly counts what a previous
  process admitted.
- **A `resume_references` that believes a frame before the chain is
  checked**, or a `Deserialize` derive returning to `DataReference` or
  `RevisionRecord`.
- **A campaign id minted from the cycle alone.** `campaign_id` carries the
  log's sequence; a restart with a per-process id collides silently.
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
- **A provenance inferred from the configuration again**, or a `sense`
  that accumulates records across its arms before observing, or a base
  URL check that refuses only `https`. Each was the shape the third
  review found, and each has a test that fails on the mutation.
- **A retained-chain check that reads a file's gap as an eviction.** The
  gap tolerance is safe only because `open_with_capacity` refuses a
  non-contiguous file first; removing that refusal turns a deleted line
  into a silent eviction.
- **An inspection that creates the file, opens it for append, takes the
  exclusive lock, or accepts an append.** Each returns `qip replay` to a
  state a review found: a checker that cannot read an archive, or that
  reads a node's journal mid-append, or that leaves an empty file at a
  typo, or that holds a chain the file does not. Three tests hold the
  first three properties and the first of them holds the fourth. The
  read-only-storage half is asserted only where the runner is
  unprivileged, because uid 0 ignores the mode bits: the test probes
  whether the mode bits bind it and prints which half it could prove, so
  on a root build host it proves that the inspection loads and no more,
  and the whole property is proven on an unprivileged runner such as
  `ci.yml`'s `ubuntu-latest`. What the checkout shows is the probe and
  the stderr line; it does not show a run.
- **A second URL parser in front of the transport, a resolver name
  admitted as loopback, a gate that one credential-bearing address does
  not go through, or a refusal that echoes the address unredacted.** The
  gate's host must be the host the transport connects to, by
  construction, the literal with a written port is the only spelling
  Terraform admits, and the gate is only a gate if every address that
  carries a credential passes it; a hand parser or a `localhost` arm
  re-opens the userinfo case and the hosts-file case, a prefix check on
  one variable re-opens both for that variable, and a refusal that prints
  `http://svc:TOKEN@…` is the leak it exists to prevent.
- **The journal's ledger and checkpoint written as two values again.**
  Any second `put` between them is the crash gap; the store in the
  billing test refuses exactly the write a split would leave alone.
