# 0050 — What an option-quote source must satisfy before the volatility surface has a caller

**Status:** *proposed*, 2026-09-06. **No vendor is named as chosen and no
vendor's terms are accepted, agreed, or characterised as evaluated.** Reading a
vendor's terms is the owner's act
([ADR 0040](0040-the-owner-authorises-the-agent-to-apply-dev-and-what-that-authorisation-cannot-reach.md)),
and this record does not do it for any source. What it settles is the
*question set* an option-quote source must answer before
`qip-data-finder`'s catalogue can hold an entry for it, and the classes of
source those questions sort into.

**Relates to:** [ADR 0034](0034-the-first-market-data-and-prediction-sources.md)
(the first sources, candidates only, licensing gate as a precondition),
[ADR 0041](0041-venue-registration-is-one-operators-attributed-click-and-never-anonymous.md)
(a source needing an account needs an attributed registration record),
[ADR 0029](0029-the-normaliser-is-removed-rather-than-recorded-as-research-only.md)
(the precedent for deleting an unreached component rather than pretending it is
wired — considered below and not followed),
[ADR 0007](0007-exact-attribution.md) and
[ADR 0023](0023-real-trading-is-the-destination-and-the-opening-is-gated.md)
(why `Usage::Trade` is asked of a platform that trades only on paper).

**Does not touch:** the paper-trading boundary; the instrument set the platform
may trade; any deployed configuration. Admitting a *data* source has never
admitted an *instrument*, and this record says so twice for a reason.

---

## Context: an engine with no caller, and why that is not neglect

`backend/crates/libs/qip-market/src/volatility.rs` is the implied volatility
surface — "smiles, skew, term structure, forward volatility and dispersion
(blueprint §16.1, engine 13)" (`:1-2`). It is complete, tested and careful: it
refuses to extrapolate rather than flat-extending a wing, because "a 10-delta
wing vol returned as though it had been observed is indistinguishable,
downstream, from one that was" (`:17-23`), and it refuses two implied
volatilities for one grid node rather than dropping the loser (`:170-172`).

Nothing calls it. The only mention outside its own crate and tests is a
doc-comment cross-reference in `qip-kernel/src/valuation.rs:221`, cited as the
example of a guard placed at the composition point. The gap map already scores
this and already says why:

> six engines were built at `1107305`, four wired then and a fifth wired since,
> leaving **the volatility surface as the only one reached by nothing** — and
> for a reason that is not "nobody got to it yet". Nothing in this platform
> ingests option quotes, so wiring it is a data source with a licensing
> evaluation ahead of it.
> — `../plan/blueprint-v10.1-gap-map.md:255-261`

The corresponding hole in the object model is `OptionDetails`
(`qip-financial/src/extensions.rs:424-436`): the extension exists, `Extension`
has an `Option` arm, and the struct is constructed nowhere outside fixtures and
tests.

So the surface is not dormant through neglect. It is dormant because the input
it needs is a licensed data source, and this platform evaluates a source's
licensing posture **before** the source is used, in `qip-data-finder`, where a
research-only licence never reaches the catalogue. That ordering is the whole
reason a record comes before a connector.

---

## The gate as it exists, in the order it fires

Any option-quote source meets the same machinery every source meets. Stated
from the code so that a proposal can be checked against it rather than against
a summary (`qip-data-finder/src/admission.rs`):

1. **A catalogue entry, or refusal by name.** No entry means "its terms have
   not been read and it is refused. Evaluate the source's terms, write the
   entry, and have it reviewed — the catalogue is code on purpose" (`:274-279`).
2. **The manifest's class and the catalogue's `expected_class` must agree.**
   "Two claims about one licence disagree, so neither is treated as current"
   (`:281-288`).
3. **Both `REQUIRED_USAGES` must be permitted** — `Usage::Derive` *and*
   `Usage::Trade` (`:67`). Derive because that is what the loop factually does;
   Trade because the loop *is* the trading path, and a source admitted on a
   research-only licence "would be promoted onto live trading by nothing more
   than the ceiling changing" (`:12-19`). **This is stricter than ADR 0034's
   own sentence**, which said a research-only licence "may be entirely
   sufficient here"; the code took the stricter reading and the code is what
   runs. An option-quote source is judged by `REQUIRED_USAGES`, not by that
   sentence.
4. **The source must be on `KNOWN_SOURCES`**, so a catalogue entry cannot
   evaluate something the feed cannot open (`:296-300`).
5. **The registration question.** Keyless, or an ADR 0041 record naming the
   operator who accepted the venue's terms. Every class of option source below
   except one needs an account, so for options this is the usual case rather
   than the exception.

---

## Decision, part one: six things an option-quote source must satisfy, beyond the gate every source meets

These are what make options different from a rate table or a bar series, and
each is derived from a type in this tree rather than from general practice.

### 1. A chain reference and a quote are two records, and both are required

`SensedRecord` has a `Quote(Quote)` arm and a `ReferenceData` arm
(`qip-market-ingestion/src/adapter.rs:31-43`), and `Quote` is keyed by an
`ObjectId` (`qip-market/src/quote.rs:13-27`). A quote is therefore a statement
*about an object the platform already holds*. For options that object is a
contract — strike, expiry, type, exercise style, multiplier, settlement — which
is `OptionDetails`. **A source that publishes prices without the chain gives
this platform prices for objects it does not have**, and the connector would
have to invent the contracts to attach them to. So a candidate source must
publish, or be paired with something that publishes, the contract reference
itself.

### 2. Implied volatility is a model output, and it must be the platform's own

`OptionDetails` carries `implied_volatility` and `greeks` — fields a vendor
would happily fill. A vendor's implied volatility embeds the vendor's forward,
its discount rate, its dividend assumption and its exercise treatment, none of
which arrive with the number. A surface built from vendor IVs therefore cannot
be reproduced from this platform's event log, which is the platform's founding
property, and it would put two independent claims about one fact in the tree —
the vendor's IV and any IV the platform later computes — where the house
principle says the louder one will be wrong.

**So: ingest the quote and derive the volatility.** What a source must supply
is the contract, the two-sided quote with sizes and the instant, and the
underlying's reference price *at that same instant*. A source that publishes
only an implied volatility and no quote may be admissible for research and is
not an input to this engine.

### 3. Synchrony across the strike chain, because the type cannot check it

This is the requirement that would be missed, and the type makes it sharp:

```
VolatilitySurface::new(underlying, as_of, forward, points)
```

— one `as_of` for the whole surface, described as "the instant the quotes were
true", and **one `forward`** for every point (`volatility.rs:145-178`). A
caller who assembles points from quotes taken seconds apart, at different
underlying prices, is asserting a synchrony the data does not have, and nothing
downstream can detect it: `Smile::vol_at` interpolates in log-moneyness against
that single forward and returns a skew nobody quoted. The surface refuses to
extrapolate and cannot refuse to be built from a smear.

**So a candidate source must either publish a chain snapshot stamped with one
instant, or publish a per-quote instant the connector can bucket into one** —
and the manifest must say which, because the bucketing tolerance is a
modelling decision, not a transport detail.

### 4. The declared publication delay must be the vendor's real one

The connector manifest carries `publication_delay_ms`, and the shipped example
is not decorative: `frankfurter-ecb-reference-rates.json:32` declares
57 600 000 ms, and the live run showed the consequence — a rate true for
2026-09-04 became knowable at 16:00Z that day and was ingested two days later,
with the test asserting `event_time + publication_delay <= the horizon it was
released at` (`../ops/live-source-frankfurter-2026-09-06.md:91-113`). **A
delayed options feed declaring a zero delay is point-in-time leakage**, which
the data rule names first among prohibitions, and it is invisible in a backtest
that looks good.

### 5. The class chosen constrains the console before anyone writes a page

`LicensingClass::allows_raw_display` is true for `Public`, `Internal` and
`Synthetic` and false for `Licensed` and `Restricted`
(`qip-financial/src/quality.rs:29-33`). Most option-quote sources land in the
false half: derived values may be shown, raw quotes may not. That is a
constraint on the portal, decided at the moment the catalogue entry is written
and enforced nowhere near it.

And the gate cannot enforce everything, which the catalogue already says about
the one source it admitted on public terms:

> Displaying these rates in the console without naming the ECB would satisfy
> every check in this file and still breach the terms it cites. Nothing
> displays them today; whoever first does owns that.
> — `qip-data-finder/src/admission.rs:129-134`

An option-quote entry must carry the same kind of sentence for whatever its
terms oblige — attribution, display restrictions, retention limits — naming
what the code cannot check.

### 6. Never `Synthetic`, and never a self-supplied input

`LicensingClass::Synthetic`'s `allows_production_decisions()` returns `false`
(`quality.rs:36-38`). A surface fitted to the platform's own model is
`Synthetic` and is structurally barred from a real decision. This is the same
discipline as ADR 0006's classical baseline, in the other direction: **the
engine cannot be given a caller by manufacturing its input.**

---

## Decision, part two: the classes of source, and what each costs in licensing terms

Described generically. No vendor is named, no vendor's terms are characterised,
and nothing here is legal advice or an evaluation.

**(a) The venue of record's own quote feed, or its consolidator.** The full,
synchronous chain at a known instant — the only class from which requirement 3
is easy rather than approximate. Costs: a negotiated contract; fees typically
structured per use and split between display and non-display, so requirement 5
becomes a billing question as well as a compliance one; usage reporting and
audit obligations that count seats, devices or applications; redistribution
refused; and terms that bind an identified account holder, so ADR 0041's
attributed registration is mandatory rather than optional. Realistic class:
`Restricted`.

**(b) Quotes bundled with a brokerage account**, where the data terms arrive
with the account agreement. Costs: the platform's data licence becomes a
function of a commercial relationship it must maintain, and the terms change
when the account does; the entitlement usually names internal use only. ADR
0034 already reasons about this shape for equities and is explicit that the
terms are unread. Realistic class: `Restricted` or `Licensed`.

**(c) A redistributor or aggregator** reselling one or more venues. Costs: two
sets of terms to read — the vendor's and the pass-through obligations of each
venue behind it — and, commonly, a limit on retaining raw quotes, which
collides directly with the event log being the record of what the platform saw.
A retention limit the log cannot honour is a reason to refuse the source, not a
reason to trim the log.

**(d) Delayed or end-of-day published series** — settlement prices, official
closes, open interest — published by a venue or a public body as reference
data. Costs: usually the lowest, sometimes attribution-only, occasionally none;
the obligations are the kind requirement 5 names and code cannot enforce.
Limits: the surface it supports is an end-of-day surface, it cannot inform an
intraday decision, and requirement 4's declared delay is large and must be
honest. Realistic class: `Public` or `Internal`.

**(e) The platform's own model.** Refused for production decisions by
requirement 6. Listed so it is refused explicitly rather than by omission.

---

## Decision, part three: the first candidate evaluated should be of class (d)

Not because it is the best surface — it is the weakest — but because it is the
only class that can prove the whole path before a commercial conversation
exists. The precedent is exact and it is this session's:
`api.frankfurter.dev` republishes the ECB's daily reference rates, is public,
keyless and needs no account, and it is the only source that has ever crossed
this platform's full path end to end:

> admitted under licence `ecb-reference-rates-via-frankfurter` (class Public)
> for derive and trade at 2026-09-06T12:10:28.362Z; keyless; no registration
> needed
> — the process's own start-up banner, `../ops/live-source-frankfurter-2026-09-06.md:29`

It got there **because it had no licensing barrier**, and its record is equally
clear about what that does not prove: "Nothing is deployed … This is one poll,
twice. Not a stream, not seven days, not an SLA" (`:192-200`). The same
argument is ADR 0034's ordering argument — prove the transport before the
commercial relationship — applied to a harder instrument.

What class (d) buys for options, concretely: the connector, the manifest, the
chain-reference record, the IV derivation of requirement 2, the synchrony rule
of requirement 3 (trivially satisfied by a settlement snapshot, which is one
instant by construction) and the bitemporal stamps of requirement 4 can all be
built and tested against a source nobody has to sign for. What it does not buy
is a surface anyone should size against: an end-of-day surface answers
end-of-day questions. **Saying so is the point.** A platform that wired engine
13 to a settlement snapshot and then quoted "the volatility surface is live"
would have manufactured exactly the impression this repository exists to
refuse.

---

## The alternatives, and why they were not taken

**(a) Wire the surface to synthetic volatilities so the engine has a caller.**
Rejected on requirement 6, and on the shape: an engine fed a number the
platform invented reads as a capability and is not one — the
`MaxExpectedShortfall` pattern the risk rules name by example, moved from a
limit to a valuation.

**(b) Delete the surface, following ADR 0029's precedent.** Considered
seriously: ADR 0029 deleted `qip-normalization` rather than record it as
research-only, because nothing constructed it, its guard could not fire, and
four documents cited it as a control. The distinction is the third clause.
**Nothing cites the volatility surface as a control**; it makes no claim
anything relies on, it refuses to extrapolate, and its absence of a caller is
recorded in the gap map rather than disguised. It is dormant, not deceptive,
and deleting it would throw away the one component that will be needed on the
day the data question is answered. Revisit if a document ever starts citing it
as a capability.

**(c) Accept a research-only option licence, on the argument that this
platform trades only on paper.** Rejected on `REQUIRED_USAGES`: the ceiling is
the only thing standing between the loop and a live path, ADR 0023 records live
trading as the destination, and a source whose licence is sufficient only while
the ceiling holds is a licence breach waiting on a configuration change.

**(d) Take the vendor's implied volatilities and greeks, because the fields
exist.** Rejected on requirement 2. The fields existing is the trap, not the
argument.

**(e) Name a vendor now and start the evaluation.** Refused. ADR 0040 leaves
the reading of a vendor's terms with the owner; ADR 0034's three candidates
have been sitting behind exactly that step since 2026-09-04, and adding a
fourth unread name would grow the queue rather than the platform.

**(f) Build the connector first and decide the licensing after.** Rejected: it
is the ordering the data rule forbids, and `admission.rs` enforces the order in
the code path rather than in anyone's memory. It is also how a
`LicensingPosture::Ambiguous` entry ends up shipped, which is where the Kalshi
and Alpaca connectors already are — code complete, terms unread, refused by the
gate, and correctly so.

---

## Where this sits in the layering

A source enters through `qip-market-ingestion` (service) as a manifest and a
connector; the licensing evaluation lives in `qip-data-finder` (service) so
every composition root asks the same catalogue the same questions; the
composition root (app) admits before it opens; the surface itself is
`qip-market` (lib) and stays a pure function of the points it is handed.
Direction: app → service → lib, inward. **No lib gains I/O** — the surface
never learns what a vendor is — and no service gains a dependency on the
runtime. The one new edge any implementation would add is a connector module
inside an existing service crate, which adds no crate and no dependency: this
record admits nothing to `Cargo.toml`.

---

## What it costs

- **Engine 13 stays without a caller for as long as this is unanswered**, and
  the gap map's count of built-unwired components does not improve. That is the
  honest price of putting licensing before use, and it is the same price ADR
  0034 pays for the equities feed.
- **Requirement 2 is work.** Deriving implied volatility in-tree means a solver
  and its conventions — forward, discount, dividends, exercise style — each of
  which is a decision that will need to be written down where the surface can
  be read against it. Taking the vendor's number would be a day's work; this is
  not.
- **Requirement 3 will make some sources unusable that look usable**, because a
  per-strike quote stream with no snapshot cannot be bucketed honestly at wide
  spreads and thin wings, which is exactly where an options desk wants the
  surface.
- **Class (d) first means the first surface is weak**, and a weak surface
  invites the sentence "we have a volatility surface" in a context where it
  should not be said.
- **A retention limit could refuse a source outright.** Under class (c) the
  common term is a cap on retaining raw quotes; the event log is append-only
  and hash-chained, so the platform would have to refuse the source rather than
  edit its record. Naming that now is cheaper than discovering it mid-contract.

## What would make this wrong

- **A source being admitted whose terms nobody read.** The single failure this
  record exists to prevent. The catalogue is code and its entries are reviewed
  like code; an entry whose evidence is a summary of a summary is the defect.
- **An `OptionDetails` appearing in the tree outside fixtures with a vendor's
  `implied_volatility` in it.** That is requirement 2 breached, and every
  surface built from it is unreproducible from the log.
- **A `VolatilitySurface` constructed from points with more than one true
  instant.** Requirement 3. The type will accept it and nothing downstream will
  notice, so the check belongs in whatever assembles the points and must be
  tested there.
- **This record being cited as authority to trade an option.** It admits a data
  question, not an instrument. There is no options order path, `Cell` has no
  constructor above paper, and nothing here changes either.
- **The gate being relaxed to admit an options source.** If an entry ever needs
  `REQUIRED_USAGES` softened, or the class-agreement check bypassed, the answer
  is that the source is not admissible — not that the gate is too strict.
- **A vendor's terms changing after admission.** The catalogue entries say a
  change in terms is a change to the entry, reviewed like code. For a class (a)
  or (b) source those changes arrive by email to an account holder, not by a
  commit, and nothing in this repository watches for them. That is an open
  operational gap this record names and does not close.
