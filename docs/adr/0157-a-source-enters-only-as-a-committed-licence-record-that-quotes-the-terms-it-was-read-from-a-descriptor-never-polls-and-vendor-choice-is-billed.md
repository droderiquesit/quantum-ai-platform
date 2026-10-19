# ADR 0157: A source enters only as a committed licence record that quotes the terms it was read from, a descriptor never polls, and vendor choice is billed

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises the admission rule and the in-tree
  measurement choices below. It does not authorise choosing, licensing or
  paying a vendor, enabling the discovery cadence in a tfvars file, or
  connecting a real venue; those are listed under "Not decided here" and are
  the owner's.
- **Date**: 2026-10-19
- **Extends**: [ADR 0034](0034-the-first-market-data-and-prediction-sources.md)
  (licensing is evaluated before use; this record says what the evaluation
  must leave behind), [ADR 0050](0050-what-an-option-quote-source-must-satisfy-before-the-volatility-surface-has-a-caller.md)
  (a refused source carries its clause; an admitted one now does too),
  [ADR 0060](0060-a-source-is-probed-only-where-a-reviewed-egress-route-already-exists.md)
  (a candidate is probed only where a reviewed route exists).
- **Keeps**: ADR 0003 (paper trading), ADR 0006, ADR 0109 (no new crate),
  [ADR 0132](0132-sensed-data-shares-one-admission-header-the-source-manifest-grows-by-version-and-a-tick-reaches-the-fabric-only-inside-a-bounded-feature-digest.md)
  (a connector that cannot state an entitlement cannot build a record),
  [ADR 0139](0139-a-committed-issuer-registry-lets-a-connector-verify-a-sources-signature-and-the-evidence-fabric-credits-only-a-verified-token.md)
  (committed files, reviewed like code),
  [ADR 0144](0144-research-connector-polling-runs-as-a-scout-cloud-run-job-with-its-own-identity-and-no-trading-role.md)
  (licensing is re-asked before every poll), and
  `.claude/rules/domains/data-and-streaming.md`.

## Context

Twenty register rows ask some form of "which licensed vendor" or "which source".
They are not twenty decisions. Thirteen of them ask the owner to pick a publisher
and pay it, which this record may not do; what the repository can decide, and
had not, is what a source has to be before the platform will read it at all. The
other seven are measurement and wiring questions on sources already in tree.
Reading the code for the twenty turned up four facts the rows do not state.

**1. "Registration is not ingestion" is pinned for a narrower property than its
name.** `qip-data-finder/tests/registration_is_not_ingestion.rs` proves that a
registered source keeps the schema and a manifest hash and none of the
publisher's text. It says nothing about polling. The property the row wants
(no source is polled because it was discovered) holds for a different reason,
and nothing names that reason: `qip_data_finder::ingestion::plan_for` and
`descriptor_for` have no caller outside tests, `RestMarketDataAdapter` has no
caller outside its own tests, and `ConnectorFeed` opens only the seven names in
`KNOWN_SOURCES` and refuses the rest ("an unevaluated source is refused rather
than fetched"). A reader citing the first test for the second property is
citing the wrong guard; this record names the right ones and pins them.

**2. A refusal carries its evidence and an admission does not.**
`admission::RefusedSource` holds the vendor, `terms_url`, `terms_dated`,
`evaluated_on`, the clause verbatim and the record that holds the evaluation.
`admission::CatalogueEntry` holds a `LicensingPosture`, and a `Declared`
posture is a `SourceLicense`: an identifier and a set of usages, with optional
dates. Nothing says which document the five declared entries were read from,
when, or by whom. The platform therefore documents precisely why it said no and
cannot show why it said yes, and the asymmetry runs the wrong way: an admission
is what moves a feed toward capital.

**3. The committed candidate file holds licence claims for publishers that do
not exist.** `data/datasets/source-candidates.json` has eight entries. Seven
are on `.example.com` hosts, each with `"posture": "declared"` and a stated
monthly fee (500, 1000, 2000, 750 and others) for a vendor nobody has
contracted. All seven name the egress route on port 9106, which
`infrastructure/egress/envoy.yaml` gives to the model-hosting listener, whose
matcher admits POST alone, so a probe of any of them would be answered 404
before a socket opens upstream. `qip-data-finder/tests/commerce_sources.rs`
calls them "catalogued with evaluated licensing posture (Declared, not
Undetermined)". They are fixtures wearing the posture of evaluations, which is
the failure `LicensingPosture::Declared`'s own doc comment forbids ("terms were
found, read, and mapped onto usages").

**4. Three register gaps are stale or half right.** The manifest does carry
`publication_delay_ms` (`SourceManifest` in
`qip-market-ingestion/src/connector/manifest.rs`, and every shipped manifest
sets it), contrary to DATA-6006's gap. The catalogue does state a per-candidate
cost (`SourceCandidate::cost`, a `SourceCost`), contrary to AMBIENT-7200's
"states no cost", though it states no discrimination. And a response latency
exists exactly where a socket exists: `HttpSourceTransport::request` times the
exchange and `ConnectorRuntime::health` returns it, but `PollReport` has no
latency field and `health` has no production caller, which is why DATA-2024
could not build a `HealthObservation::served`.

Locate them with
`grep -rn 'descriptor_for\|plan_for' --include=*.rs backend/crates | grep -v '/tests/'`,
`grep -rn 'RestMarketDataAdapter' --include=*.rs backend/crates`,
`grep -n 'pub const KNOWN_SOURCES' backend/crates/services/qip-market-ingestion/src/connector_feed.rs`,
`grep -n 'pub struct RefusedSource\|pub struct CatalogueEntry\|pub fn catalogue' backend/crates/services/qip-data-finder/src/admission.rs`,
`grep -c 'example.com' data/datasets/source-candidates.json` and
`grep -n 'port_value: 9106' -B3 infrastructure/egress/envoy.yaml`.

## Decision

1. **A source is read only through a licence record, and a record quotes what
   it was read from.** The record is the catalogue entry in
   `qip_data_finder::admission`, which already is the single place `admit`
   consults. A `Declared` entry must carry, beside the licence, the evidence
   `RefusedSource` carries: `terms_url`, `terms_dated`, `read_on`, `read_by` (an
   operator label, as `RegistrationRecord::operator` is), the clause that grants
   `Derive` and `Trade` quoted verbatim, the dataset it covers, and the source's
   `SourceCost`. The constructor refuses a blank field and a `read_on` after
   the instant it is built for. A new source_id needs its own record even on a
   host another record covers: a licence identifier may be shared, but the
   entry must quote the clause that covers *this* dataset, because "the same
   publisher" is how a spot ticker's terms get read as covering a depth feed
   nobody read.
2. **The five entries that predate this rule are grandfathered by a ratchet,
   not by silence.** Coinbase, ECB, NY Fed, NWS and Frankfurter are admitted
   today with no structured reading on file. Suspending them would stop the
   deep brain's node loop on its first poll (a refused poll is returned as an
   error), and nothing is deployed to protect. They take the evidence arm
   `Unrecorded`, which admits, banners the fact in
   `LicensingDecision::describe`, and may be held by exactly those five ids.
   The set may shrink as the owner back-fills a reading; it may not grow. Reading
   terms is the owner's act (ADR 0034, ADR 0040), so this record does not write
   dates and names for entries it did not read.
3. **A non-zero cost is not admitted without a named owner approval.** A record
   whose `SourceCost` has a monthly fee, a per-request charge, or a bounded
   free allowance must carry `spend_approval`, naming a committed approval
   record. Without it the entry is refused. That is a restriction, so it is
   inside this record's authority; granting the approval is not (see "Not
   decided here").
4. **A descriptor never polls.** `plan_for` and `descriptor_for` describe; no
   production code builds a connector, a `DataAdapter` or a
   `RestMarketDataAdapter` from a `RegisteredSource` or an `IngestionPlan`. A
   source reaches a running poll in one reviewed change that adds all of: the
   hand-written connector, its committed manifest, its `KNOWN_SOURCES` entry, its
   topic in `topic_for`, its egress route, and its licence record. The
   acceptance suite pins the absence of the generic path and the register
   stops citing `registration_is_not_ingestion.rs` for it. If a generic
   adapter is ever wired, it must take the `LicensingDecision` the gate
   minted as an argument, so that holding an adapter implies the gate ran;
   today there is nothing to wire it to and the absence is the guard.
5. **Candidates are written by an operator or not at all (DATA-2001).** No crawl
   or search stage is built. A candidate has to name an `egress_route`
   (ADR 0060) and a route exists only where someone wrote and reviewed an
   Envoy cluster, so a machine-generated candidate would be unreachable by
   construction, and a stage producing candidates nobody can probe would be a
   control assessing inputs it manufactured. Tooling may emit a *proposal*
   file for a person to read; a proposal is never loaded by `load_source_candidates`.
   Whether crawling a publisher is lawful is a legal reading about each
   publisher's terms and robots policy; this record neither says it is nor says
   it is not.
6. **A fixture is not an evaluation (DATA-2010).** An entry in the deployment
   candidate file whose host is an RFC 2606 reserved name (`.example`,
   `.example.com`, `.test`, `.invalid`, `.localhost`) may not carry a `Declared`
   posture. The seven commerce placeholders become `Undetermined` and their
   invented fees are removed from any scoring path. The commerce suite
   then asserts the stronger thing it should have: all seven kinds are catalogued
   and every one is refused until a record exists. No commerce connector is
   built and no commerce vendor is chosen.
7. **Social and community sources (DATA-2007).** None is admitted, and the
   rule for one is fixed now. A social or community source is surface-web only
   (`SourceTier::SurfaceWeb`); it passes the same record, route and
   registration gates; the connector drops author identifiers before a record
   exists (the platform consumes aggregates it computes, never the text of a
   private individual's post), because `personal_data` screens a floor of direct
   identifiers by field name and a handle is not on it; and its text enters
   as untrusted by type (ADR 0158). `DeepWebAdapter::admissible` is unchanged:
   `Licensed` needs a declared licence of the identifier the mode names,
   `Rendered` and `Bulk` need an enclave that admits the host, `DarkWeb` is
   refused in every mode. Court dockets and corporate registries name private
   individuals as parties and officers, so a source of that kind registers only
   when its schema screens clean or its connector reduces parties to entities
   before the record exists.
8. **Connector health is measured where the socket is (DATA-2024).** Connector
   sources do not enter `DataFinder`'s registry. They are admitted by licence,
   not discovered, `DataFinder::assess` would probe them a second time beside
   the poll that already reaches them, and the scorer reads history only for
   sources it assesses. The latency a connector can honestly report is the
   transport's own elapsed time, so the poll's `PollReport` gains the
   exchange's latency and the freshest payload instant, and the arm reports a
   served, failed or timed-out observation on its status surface. Nothing
   scores from it. For *discovered* sources the history was fed by nothing:
   `score` composes the stored observations with the probe it is scoring and
   throws the probe away, and `monitor`, the only writer, has no caller. The
   finder therefore records the probe it just scored, keyed by candidate id,
   so the next assessment reads a history it built.
9. **Claim locations come from the publisher's payload or are absent (EVID-2037).**
   `connector/evidence.rs` stamps `region:{:?}` of the manifest, a string
   `GeoPoint::parse` cannot read, so `check_geographic_consistency` reports
   `Unverifiable` for every production claim. That is honest and it is also
   vacuous. A claim carries a `geo:lat,lon` location only when the publisher's
   own payload states coordinates and the connector copies them verbatim;
   otherwise it carries none, and the source region moves to a metadata key. The
   NWS station observations already in tree are such a source: the payload's
   `geometry` is the place the reading was taken, its licence is already
   admitted, and the connector's note that it does not derive a *region* from
   the coordinates concerns geocoding, which copying the publisher's statement
   is not. No new source, extractor or licence is needed for the first honest
   geo-bearing claim. A coordinate extracted from free text is refused: that is
   a model's guess about a place.
10. **The price of information has a form, not a value (DATA-6006).** A net
    score is `value_per_ic_month x incremental_ic - monthly_cost`, where the
    value is declared per source in its licence record by the owner and the cost
    is the record's `SourceCost`. A source with a non-zero cost and no declared
    value has no net score and is never selected, polled for research or
    recommended: absence is "unpriced", not zero. A free source needs no value,
    since its cost is zero and the gross score ranks it. Delay is not a second
    parameter. `SourceAlphaDesk` aligns a source's signal with the outcome at
    the instant the platform could first have read it, so a source that
    publishes late scores lower on the same outcome through alignment, and a
    discount factor would be a number nobody can measure applied on top. The
    desk reads `SourceCost` and `publication_delay_ms` from the record and the
    manifest, not from a source id alone.
11. **The acquisition menu admits a catalogued candidate only with a measured
    gain (AMBIENT-7200).** `cheapest_viable` keeps its two fixed options and gains
    one entry per catalogued candidate that has (a) an admitted licence record,
    (b) a stated cost (it has one) and (c) a `SourceAlphaDesk` score for that
    source. Discrimination is the measured incremental information, never a
    declared figure; a candidate with no measurement is not on the menu. A free
    source's cost on the ranking scale is the loop's own cost of making one
    request (`RESWEEP_COST_BP`), since `record_acquisition` refuses a zero cost
    and a free request is not free of the platform's work. The scout hand-off still
    reaches nothing while the discovery cadence is null in every environment.
12. **An open research question is a gap the market already prices (REASON-3003).**
    The chain gap, then research item, then market grant, then acquisition or
    campaign is the planner; `research_plan.rs` records why a sweep's started items
    fit its goal. A second planner launching campaigns from "questions" would
    duplicate the market's pricing and bypass its budgets. Data spend is bounded
    by `acquisition_budget_bp` and by decision 3; a campaign touches only
    admitted sources. The cap's value is not decided here.
13. **The edge node's feed stays simulated (EXEC-10147).** `FEED_VARIABLE`
    accepts exactly `simulated`. A live market-data source for the node is a
    real-venue connection, market-data-only or not, and is not decided here.
    The deep brain's connector arm, which reads public research data through
    the egress proxy, is a different path and is unaffected.
14. **Regulated wagering needs only a citation (EVENT-7145).** `AccessBook` in
    `qip-prediction/src/access.rs` is empty by default and refuses everything,
    gives `VenueKind::Wagering` its own entry point, and refuses an order sent
    down the wrong path rather than re-routing it. The register can cite it.

## Consequences

- An admission can be audited by a person against the document it quotes, as a
  refusal already can, and a licence cannot be "declared" by an edit that quotes
  nothing.
- The five existing sources keep working and are visibly different from a source
  read under this rule, until the owner back-fills them.
- The deployment candidate file stops asserting licences for publishers that do
  not exist, so the commerce suite becomes a refusal test.
- The finder's history becomes real for discovered sources, which makes a second
  assessment of the same candidate differ from the first: that is the point
  and it is a visible behaviour change.
- Code slices left, each with a mutated test: the evidence arm and constructor
  with the `Unrecorded` ratchet and the cost/approval refusal (DATA-2002), the
  absence pin for `descriptor_for`, `plan_for` and `RestMarketDataAdapter`, the
  reserved-host refusal and placeholder flip (DATA-2010), the finder recording
  its probe (DATA-2024), the NWS location (EVID-2037), the net score and
  unpriced refusal (DATA-6006), and the acquisition menu (AMBIENT-7200).

## What it costs

- **A record is longer than a label.** The next source is a bigger review than
  the last five were, and the owner must read terms and sign a name and a date to
  admit one. That is the point and it is slower.
- **Five entries stay admitted on a reading nobody can show.** The ratchet
  bounds the exposure, it does not remove it. Anyone who reads `Unrecorded` as
  "evaluated" has the original problem.
- **No discovery path.** The platform will not find a useful publisher by itself.
  Every source the desk ever reads is one a person named, found, read and
  routed. The informational edge the blueprint places in the deep web is
  reachable only as fast as an operator can list it.
- **Net scoring is unavailable for paid sources until the owner declares a
  value.** A paid source that would have won on gross score now never competes.
  The alternative was selecting on a number somebody chose without declaring
  it.
- **Moving connector health out of the finder** leaves the scorer's reliability
  term blind to the seven shipped sources. Their reliability is an operational
  fact in the node's status, not a ranking input.

## What would make this wrong

- A discovered-source pipeline whose candidates demonstrably need no reviewed
  route (for instance, a tier of sources reachable without the proxy). Decision 5
  rests on ADR 0060's premise that every route is reviewed; if that premise
  changes, so does this.
- Observed: a `Declared` record whose quoted clause does not say what the
  record claims it grants. The record makes this checkable by a person; it does
  not make it checked. If one ships, the remedy is a second reader on every
  admission, not more fields.
- Observed: the finder's recorded probes pulling a healthy source's reliability
  down because its HEAD endpoint is slower than its data endpoint. Decision 8
  would then need the observation taken from a data request, not a HEAD.
- A publisher whose coordinates are wrong in its own payload. Decision 9 copies
  the publisher's statement and makes the check meaningful, not the statement
  true.

## Dissent

The strongest counter-position is that a generic descriptor-driven adapter is
the only way the platform scales past a hand-written connector per source.
Discovery already produces a `RegisteredSource` with a mechanism, a poll plan,
a minimum interval, a schema fingerprint and a credential requirement
(`IngestionPlan`), and `RestMarketDataAdapter` already turns a vendor's JSON into
`SensedRecord`s with explicit limits. Seven connectors in a year is a rounding
error against the long tail the blueprint describes; each hand-written one is
a commit, a manifest, a fixture and a review, and the review mostly checks the
parser, not the licence. A generic adapter over a licensed, probed, scored,
schema-fingerprinted source would be safer than most of those parsers because it
would be one parser, tested once.

It lost on three grounds. A parser is not the dangerous part: the dangerous part
is a number arriving that nobody here computed, whose meaning (units, adjustment,
session, revision) a schema fingerprint does not carry, and a generic decoder
must guess it. The licence gate is per dataset (decision 1) and a generic
adapter cannot know which dataset a URL serves. And the platform has no
deployed process with an outbound path, so scaling the number of sources is not
today's constraint; reading their terms is. It is revived by a measured
backlog of *evaluated, recorded* sources waiting only on parser work, at which
point a decoder generated from a committed manifest (not from discovery) is the
design to argue, taking the gate's decision as an argument.

## Code it constrains

- `qip-data-finder/src/admission.rs`: `CatalogueEntry`, `catalogue`,
  `admit_from_registered`, `LicensingDecision::describe`, `RefusedSource`,
  `StandingAdmission` -
  `grep -n 'pub struct CatalogueEntry\|pub fn catalogue\|pub fn admit_from_registered\|pub struct RefusedSource\|pub struct StandingAdmission' backend/crates/services/qip-data-finder/src/admission.rs`.
- `qip-data-finder/src/legal.rs`: `LicensingPosture`, `SourceLicense` -
  `grep -n 'pub enum LicensingPosture\|pub struct SourceLicense' backend/crates/services/qip-data-finder/src/legal.rs`.
- `qip-data-finder/src/ingestion.rs`: `plan_for`, `descriptor_for` (descriptions
  only) - `grep -n 'pub fn plan_for\|pub fn descriptor_for' backend/crates/services/qip-data-finder/src/ingestion.rs`.
- `qip-data-finder/src/finder.rs`: `DataFinder::assess`, `score`, `observe_health`,
  `monitor`, `health_observations` -
  `grep -n 'pub fn assess\|fn score\|pub fn observe_health\|pub fn monitor\|pub fn health_observations' backend/crates/services/qip-data-finder/src/finder.rs`.
- `qip-data-finder/src/catalogue.rs`: `load` (reserved-host refusal) and
  `data/datasets/source-candidates.json`.
- `qip-data-finder/src/source_alpha.rs` and `qip-deepbrain/src/source_alpha.rs`:
  `SourceAlphaHistory::score_over`, `SourceAlphaDesk::score` -
  `grep -n 'fn score_over\|fn score' backend/crates/services/qip-data-finder/src/source_alpha.rs backend/crates/apps/qip-deepbrain/src/source_alpha.rs`.
- `qip-market-ingestion/src/connector_feed.rs`: `KNOWN_SOURCES`, `topic_for`,
  `ConnectorFeed::poll_referencing`; `connector/runtime.rs`: `PollReport`,
  `ConnectorRuntime::health`; `connector/evidence.rs`: the `with_location` call;
  `rest.rs`: `RestMarketDataAdapter`; `connectors/nws_station_observations.rs`.
- `qip-evidence/src/evidence.rs`: `check_geographic_consistency`, `GeoPoint::parse`.
- `qip-deepbrain/src/research.rs`: `cheapest_viable`, `RESWEEP_COST_BP`;
  `connectors.rs`: `ConnectorArm::poll`; `discovery.rs`: `DiscoveryDesk::maybe_run`.
- `qip-edge-node/src/feed.rs`: `FEED_VARIABLE`; `qip-prediction/src/access.rs`:
  `AccessBook`.

## Rows unblocked

- **R12-DATA-2024** - decided, code: connectors stay out of the finder's registry;
  the transport measures latency and `PollReport` carries it; the finder records the
  probe it scores. Register: the gap becomes "connector poll health is reported on
  the node status, not scored".
- **R12-DATA-2001** - decided, also PROTECTED: no crawl or search stage; candidates
  are operator-written; enabling `deepbrain_discover_every` is not done here.
  Register: restate.
- **R12-DATA-2002** - decided, code: no generic adapter; the record carries the
  terms it was read from; the absence is pinned. Register: cites the admission
  gate and the absence test, not `registration_is_not_ingestion.rs`.
- **R12-DATA-2005** - BILLED vendor choice, rule decided: depth adapter stays
  unwired; every new acquisition needs a record.
- **R12-DATA-2006** - BILLED vendor choice, rule decided: party names are personal
  data; entity-level only.
- **R12-DATA-2007** - BILLED vendor choice, rule decided (decision 7); nothing built.
- **R12-DATA-2010** - BILLED which source, rule decided, code: placeholders become
  `Undetermined`.
- **R12-DATA-1153** - BILLED vendor choice; the eight alternative-data stubs are not
  on `KNOWN_SOURCES` and cannot be opened; each needs a record and a route.
- **R12-TICK-1175** - BILLED vendor choice; options are refused on the evidence of
  ADR 0050; futures, funding and liquidations need records.
- **R12-TICK-2045** - BILLED vendor choice, as TICK-1175.
- **R12-TICK-2046** - BILLED vendor choice; crypto depth is the first candidate to
  read terms for, on a host whose spot ticker is already admitted.
- **R12-EVENT-7145** - BILLED logistics vendor, rule decided; wagering isolation is
  `AccessBook` and needs only citation.
- **R12-EXEC-10147** - paper-boundary and BILLED; the feed stays simulated.
- **R12-REASON-3003** - decided: the gap, item, grant chain is the planner; spend cap
  is the owner's.
- **R12-REASON-3012**, **R12-REASON-3013**, **R12-REASON-3016** - BILLED vendor choice,
  rule decided: an analyst for a domain is written when a source with a record exists,
  not before.
- **R12-EVID-2037** - decided, code: coordinates from the publisher's payload or none.
- **R12-AMBIENT-7200** - decided, code: the menu admits recorded, measured candidates.
- **R12-DATA-6006** - decided, code, also BILLED for any declared value: net score
  form, unpriced refusal, delay by alignment.

## Candidate publishers to evaluate first

Named only as places to read terms. **Nothing in this list has been read, and
none is admitted, recommended or priced by this record.** Each needs its own
record under decision 1, each fails closed until it has one, and a paid
product from any of them is the owner's to choose.

- Public-record and government publishers (a record may be easier here because
  the terms are usually statutory or an open licence): US Treasury sanctions
  lists and FiscalData, the US Census foreign-trade series, UN trade statistics,
  national statistical offices and election authorities (REASON-3012); US
  energy and commodity agencies' published price series, central-bank and
  statistics-office series (TICK-2045, TICK-1175 futures curves).
  The SEC's EDGAR was tried and could not be read on 2026-09-20 (see the
  comment on `RefusedSource` in `admission.rs`), so it is not admissible until
  someone reads its terms.
- Courts, procurement, patents, registries (DATA-2006): national patent offices
  and procurement portals publish openly in many jurisdictions; court dockets
  and corporate registries often name individuals (decision 7) and some
  court systems charge per page, which is BILLED.
- Crypto depth (TICK-2046): the order-book endpoint of the venue whose spot ticker
  is already admitted. Whether the admitted terms cover book data is the question,
  and decision 1 forbids assuming the answer. Perpetual funding and liquidations
  belong to different products with different terms.
- Options (TICK-1175, TICK-2045): four publishers were read on 2026-09-19 and refused
  on their own clauses (`deribit-options`, `cboe-delayed-option-quotes`,
  `okx-options`, `hkex-option-daily-reports`), and two could not be read. Nothing
  new is admissible without a licence the owner purchases.
- Logistics, shipping, customs, satellite, IoT (DATA-1153, EVENT-7145, REASON-3016,
  REASON-3013): public AIS aggregators and customs statistics are named in the
  stubs under `connectors/`; their terms are unread, and the commercial vendors
  are BILLED.

## Not decided here

- **BILLED** - choosing, contracting or paying any vendor, data subscription or
  licence for: licensed exchange feeds and depth (DATA-2005, TICK-2046),
  court/procurement/patent/registry products (DATA-2006), social and news
  vendors (DATA-2007), commerce data (DATA-2010), satellite/IoT/shipping/customs
  (DATA-1153), options/futures/funding/liquidation feeds (TICK-1175, TICK-2045),
  logistics (EVENT-7145, REASON-3016), geopolitical and guidance/capital-structure
  series (REASON-3012, REASON-3013). Also: the approval record decision 3
  requires; any declared `value_per_ic_month` that could make a paid source
  selectable (DATA-6006); the research-campaign data-spend cap (REASON-3003).
- **PROTECTED** - setting `deepbrain_discover_every` or `source_candidates_file`
  in `infrastructure/environments/*/terraform.tfvars` (DATA-2001, AMBIENT-7200);
  any new `egress` route or Envoy cluster for a source. The need: a non-empty
  candidate file containing only recorded sources, and a reviewed route for each.
- **paper-boundary** - a live market-data source for the edge node (EXEC-10147).
  `FEED_VARIABLE` stays `simulated`; connecting any real venue's feed, read-only or
  not, is the owner's with an ADR amending the boundary's documentation.
- **RISK** - none created, changed or loosened.
- **Owner reading of terms** - the legality of crawling a publisher, and the
  back-fill of `read_on` and `read_by` for the five grandfathered sources, are
  the owner's acts and are not performed by this record.
