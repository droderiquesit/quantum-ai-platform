# ADR 0034 — The first market-data and prediction sources

- **Status:** accepted, subject to the licensing gate below
- **Date:** 2026-09-04
- **Decides:** plan rows D9 (the equities/crypto vendor) and the Phase 6
  prediction source
- **Relates to:** ADR 0024 (the egress proxy), ADR 0003 (paper trading),
  `.claude/rules/domains/data-and-streaming.md` (licensing evaluated before use)

## The decision

Three sources, in this order, each added to `egress_allowed_upstreams` and to
the Envoy bootstrap in the same commit:

1. **Crypto — Coinbase Exchange public market data.** First, because its
   public endpoints need no key, no account and no contract, so it can be
   proven end to end through the egress proxy before any commercial
   conversation happens. It is a US-regulated venue with published terms and
   a documented REST API.
2. **Equities — Alpaca Market Data.** The primary target for the Phase 2
   gate. Chosen over the alternatives because it is the only candidate whose
   market-data terms and whose *paper-trading brokerage* are the same
   account: this platform never submits a live order, and a vendor whose
   free tier is built for exactly that posture is one whose terms are least
   likely to be violated by what this platform does.
3. **Prediction — Kalshi.** A CFTC-regulated designated contract market with
   a documented public API. Chosen over offshore prediction venues because
   the regulatory posture is legible and the terms are published, which is
   the same criterion applied to the other two.

`api.frankfurter.app` stays as the FX reference-rate source it already is.
It is not a substitute for any of the above: the Phase 2 gate is about the
equities universe this platform sizes against, and a family surviving a
holdout of FX reference rates answers the gate for FX and nothing else.

## The licensing gate is a precondition, not a formality

**No source named here may reach the catalogue until
`qip-data-finder` has evaluated its licensing posture and admitted it.** That
gate exists, refuses a research-only licence, and is tested. This record
selects candidates; it does not certify their terms.

Stated plainly because it matters: the terms above are described from each
vendor's published documentation as a basis for choosing what to evaluate.
**They have not been read against a contract by anyone, and this record is
not legal advice.** Two properties must be confirmed before each source is
used, and confirmed against the vendor's own current terms rather than
against this paragraph:

- **Internal research use is permitted.** This platform consumes data to make
  paper-trading decisions and never redistributes it. That is the narrowest
  possible use and the one most likely to be permitted, but "most likely" is
  not the standard.
- **Whether the licence is research-only, and whether that matters.** Because
  the platform never trades live and never resells, a research-only licence
  may be entirely sufficient here where it would not be elsewhere. The
  evaluation should record which it is rather than treat research-only as
  automatically disqualifying.

If a source fails its evaluation, it is refused and the next candidate is
evaluated. That is the gate working, not a setback.

## Why these, and why in this order

The order is chosen so that **the transport is proven before the commercial
relationship is**. Every one of the last several sessions' failures has been
a wiring failure — an unordered IAM grant, a reserved variable name, a
credential the process could not read — and none of them needed a vendor to
surface. Coinbase's keyless public endpoints let the whole path be proven
(allowlist, Envoy cluster, connector, licensing gate, bitemporal record)
against a real remote host with nothing to sign. When Alpaca is added, a
failure is then unambiguously about Alpaca.

On the equities choice specifically: the alternatives considered were
Polygon.io, Tiingo, Finnhub, Twelve Data and Nasdaq Data Link. All are
plausible and several are better on raw coverage. Alpaca wins on the
criterion this platform actually optimises for, which is not coverage but
*legibility of posture* — the same account is the paper-trading broker, so
"what this platform does with the data" and "what the vendor expects it to
do" are the same sentence. `qip-brokers` already has a simulated broker; a
vendor whose sandbox is the intended destination is a shorter path than one
where market data and execution are unrelated products.

## What it costs

- **Three vendors is three supply chains.** Each is a host in the allowlist,
  a cluster in the bootstrap, an availability dependency and a terms document
  that can change under the platform without notice.
- **Two of the three need credentials**, which means two more secrets, mounted
  as files through `qip_core::secret` — never as environment values, since
  these are built binaries and ADR 0031's exception does not reach them.
- **The equities feed is the one that can cost money.** Free tiers have rate
  limits and delayed or venue-limited data. A Phase 2 gate argued on delayed
  data is a weaker claim than one argued on consolidated real-time, and the
  difference is a subscription. This record does not commit to that spend;
  it commits to starting on the free tier and stating in the gate evidence
  exactly which data fed it.
- **Kalshi's universe is small and its contracts are idiosyncratic.** The
  Phase 6 gate will be argued over a narrow set of markets. That is a real
  limitation on how far the result generalises and belongs in the gate
  evidence rather than being discovered by a reader.

## What would make this wrong

**A licensing evaluation that fails.** If any source's terms forbid what this
platform does, it is refused. This record names candidates; the gate decides.
Nothing here overrides it and nothing here should be read as having
pre-approved a source.

**Choosing the vendor before proving the path.** If Alpaca is wired before
Coinbase has demonstrated a request and a response through the allowlist, the
ordering argument above has been discarded and the next failure will be
ambiguous between transport and vendor.

**If the equities free tier turns out to be too thin to argue a gate on.**
That is not a reason to argue the gate anyway on data that cannot support it.
It is a reason to come back for a subscription decision, with the specific
limitation named.

**If any of these ever becomes an execution venue.** These are data sources.
Alpaca in particular offers a brokerage, and the fact that its paper
environment is *why* it was chosen makes this the most likely place for the
paper-trading boundary to be tested by accident. It must not be. ADR 0003 is
absolute, the three enforcement layers stand, and a data credential must
never become an order credential.
