# Algorik Master Blueprint v10.1 — full source text

**Provenance.** Extracted verbatim (paragraph text only — tables flattened
row-by-row, headings kept as their own lines) from
`Algorik_Master_Blueprint_v10.14.docx`, supplied to this session on
2026-09-03. The document's own header and footer both read "Version 10.1",
matching ADR 0022's own naming — "The Algorik Master Blueprint v10.1-4,
together with its companion diagram" — so the `.14` in the filename is a
save-revision counter, not a distinct version 10.14.

**Why this file exists.** Every numbered section this repository already
cites (`§6.2`, `§18.1`, `§25`, `§37`, `§37.4`, `§40`, `§41.4`, `§41.5`,
`§41.6`, and others across `CLAUDE.md`, the ADRs, and the rules under
`.claude/rules/`) refers to sections of *this* document — confirmed by
matching section numbers and titles below against those citations (`37.4
Custody`, `41.5 The Shipping Payload`, `41.6 Service Catalog`, `18.1 The
Feasibility Gate`, `6.2 Degradation Order`, all present). Before this file,
the only blueprint artifact committed anywhere in this repository was a
much shorter companion — the interactive HTML diagram — which is real,
distinct, and separately valid (ADR 0022 names "the blueprint... together
with its companion diagram" as two things), but is roughly 40% the length
of this document and contains no numbered sections at all. Any traceability
scoring done only against the HTML companion was necessarily scoring
against the diagram, not the ~232-section prose specification this file
now makes searchable and citable for the first time.

**What this is not.** Not a restatement, not a summary, not reformatted for
readability beyond flattening tables into row-per-line text so the content
survives plain-text extraction. Table structure is lost; the words in every
cell are not. Where a heading numbering looks irregular, that is the source
document's own numbering, transcribed as found rather than corrected.

---

ALGORIK
Cognitive Investment Platform
Master Architecture & Application Blueprint
Seven planes  ·  Observe, understand, believe, value, allocate, act, remember
Every asset class  ·  AI trained centrally, shipped outward  ·  Quantum allocation
Every application in Rust.  Every service Google Cloud or IBM.
Cognition is not a layer. It is what emerges when a system models the world, remembers, reasons counterfactually, and knows what it does not know.
Version 10.1  —  Complete end-to-end blueprint, corrected and extended with the Experience architecture. Single source of truth.
1. Executive Summary
Algorik is a cognitive investment platform, and version 10 is the first version that describes it end to end — from the customer who signs in to the venue that fills the order, and from the filing that arrives to the position it becomes eleven minutes later.
The platform observes the world through information rather than prices alone, builds an explicit model of what causes what, remembers what it has seen, holds beliefs with stated confidence, values assets that have no continuous price, allocates capital with quantum and classical optimisation, acts in microseconds through regional nodes, and learns from every path it took and every path it declined. Version 8 was a reflex arc. Version 9 made it cognitive. Version 10 makes it a whole company's platform: one architecture that carries a public website, an authenticated investor portal, mobile clients, a wallet, treasury, a global ledger, and the investment engine behind them, with one design system, one identity model, and one set of controls.
Ten things are true of the platform in version 10, and every section that follows is an elaboration of one of them.
#
Statement
What it means
1
One system, three surfaces
The public landing experience, the investor portal and the mobile client are one Rust codebase over one set of application APIs, behind one Google-managed security edge. The customer never touches the investment platform directly; the portal is a control surface over the ledger, not a trading terminal.
2
Every application is Rust; every managed service is Google Cloud or IBM
Frontend, APIs, warm services, batch jobs, solvers, training, ingestion and the execution node share one workspace and one dependency graph.
3
Intelligence is global and runs once
Ingestion, cognition, valuation, strategy lifecycle, training and risk policy live in one region and produce policy.
4
Optimisation is quantum-assisted and produces policy, never orders
IBM Quantum solves family allocation, cycle selection, path assignment and capital placement; classical solvers always run alongside; a routing gate decides what reaches a QPU.
5
Execution is regional and independent
Three identical algorik-node binaries on Compute Engine own venue connectivity, market-data normalisation, order books, features, strategies, quoting, netting, the risk gate, inventory and execution locally, in microseconds. Market streaming is Rust; nothing managed sits in that path.
6
Policy goes down, outcomes come up
A signed twelve-item payload ships from central to every region over the control fabric; fills, verdicts and observations return. A region that loses central narrows; it never halts on that alone.
7
Deterministic code holds every veto
Feasibility, risk and transfer gates are ordinary Rust with exhaustive fixtures. No model — statistical, quantum or language — has decision authority anywhere.
8
Capital moves autonomously only inside signed corridors
Humans approve where money may go, once, with a hardware key and a 24-hour delay. Machines decide when, inside caps. The Wallet sees every holding and can move none of it. Three independent controls agree before capital leaves a venue.
9
The ledger is the only truth
Cloud Spanner holds money state per user and per strategy with full attribution. Data is pass-through: streams become statistics, text becomes facts, and nothing grows with tick rate.
10
The platform is honest about capital
At two hundred dollars it is a correctness harness at five times capital per month, and it says so on the screen. The arenas where small capital wins are informational, and the roadmap is ordered around them.
Version 10 also corrects itself: cost totals now sum, the roadmap's four gates (Phases 2, 3, 6 and 8) are named consistently, the risk envelope is ten levels everywhere, the deep web is a first-class source tier with its own governance, and the position lifecycle, registries and compounding policy that version 9 left implicit are specified.
1.1 What Changed and Why — v9 over v8, and v10 over v9
Missing in v8.0
Consequence
Added in v9.0
No causal layer — everything was correlation
When a regime broke, every model broke at once, because they had all learned the same spurious structure
A causal graph over drivers, with interventional reasoning. Section 9
No episodic memory — estimators forget by design
The system could not recognise a situation it had already lived through
Compressed episodes with analogical retrieval. Section 10
No counterfactual scoring
Four hundred vetoed cycles a day carried an enormous learning signal that was discarded
Shadow execution and scoring of every path not taken. Section 12
No self-model
The system could not direct attention toward its own blind spots
Capability and uncertainty estimation driving an exploration budget. Section 13
No world knowledge
Prediction markets, event-driven strategies and private assets were unreachable
Information ingestion and an entity graph. Sections 7 and 8
No valuation without a price
Roughly sixty percent of investable wealth was structurally excluded
A valuation plane: term structure, credit, volatility surface, illiquid marks, cashflow forecasting. Section 16
No feasibility check at size
At small capital the system would spend the day detecting opportunities it could not execute
A feasibility gate ahead of the profitability filter. Section 18
No after-tax reasoning
After-tax return is the only return that exists, and it was absent
A tax engine with lot selection and jurisdiction awareness. Section 25
No source discovery
Ingestion consumed known sources. Nothing found new ones, and the platform&apos;s own venues and credentials were not watched
A discovery crawler over surface and dark web that registers sources and creates feeds, with a hard exclusion on non-public information. Section 7.4
Registries absent
Hundreds of asset classes and venues were architecturally reachable and unlisted. Divestment was a side effect of other flows
Asset class registry, venue onboarding, settlement calendars, hedge map, cross-margin model, and a position lifecycle engine. Sections 17.7, 25.6, 31.4, 32.3, 32.4, 34.4
The table above is what version 9 added over version 8 and is retained because its reasoning still governs. Version 10 adds the following.
Added in v10
Why
Experience architecture — public edge, application APIs, investor portal, mobile, admin, entitlements, frontend security boundary. Sections 40.5 through 40.14
The customer-facing product had a surface but no architecture. The website and the platform must be one system
Deep-web ingestion tier with access classes and governance. Section 7.6
The largest lawful breadth gain available, and the arena where small capital wins
Compounding policy. Section 18.4
At small capital the reinvestment schedule is a strategy, and nothing reasoned about it
Position lifecycle, asset class registry, venue onboarding, settlement calendars, hedge map, cross-margin model. Sections 17.7, 17.8, 25.6, 31.4, 34, 35
Breadth and divestment were capability claims, not governed sets
Corrected cost model, gate naming, risk-level count, closed-decision consolidation and rule numbering
A single source of truth cannot contradict itself
1.2 The Seven Planes
Plane
Function
Time Scale
Ingestion
Observes the world: news, filings, releases, chains, registries, physical indices, and prices. Resolves everything to entities.
seconds to daily
Cognition
Understands it: world model, causal graph, episodic memory, beliefs with confidence, counterfactuals, self-model, hypotheses.
minutes to days
Valuation
Prices what has no price: term structure, credit, volatility surface, illiquid marks, cashflow forecasts, corporate actions.
minutes to daily
Intelligence
Trains models, generates and statistically gates strategies, sets risk and corridor policy.
minutes to days
Optimisation
Decides allocation across families, regimes and horizons; which cycles to fund; where capital sits. Quantum and classical.
seconds to hours
Execution
Regional nodes holding shipped models, beliefs, strategies and limits, investing locally in microseconds.
microseconds
Ledger, wallet and treasury
Authoritative money state per user and per strategy. Reconciles every holding, moves capital inside signed corridors.
transactional
1.3 An Honest Word on Capital
At two hundred dollars, infrastructure at roughly a thousand dollars a month costs five times the capital every month. A round trip at ten basis points costs forty cents. Withdrawal fees on some chains exceed the position. Minimum order sizes bind. Most of the arbitrage this platform can detect is, after costs at that size, structurally unprofitable for a taker.
Two hundred dollars is a correctness harness, not an engine — real money at risk to prove the plumbing, which is exactly what the Phase 3 gate exists for — thirty days live, inside the holdout band. There is, however, one arena where small capital is an advantage rather than a handicap, and the roadmap is reordered around it.
Arena
Why small capital works there
Prediction markets
Thin books, edges that come from research rather than speed, and large firms largely absent because they cannot deploy meaningful size. Two hundred dollars is the correct size, not a limitation.
Maker rebates on quiet pairs
Providing liquidity earns rather than pays. No per-trade drag, and size is not the constraint.
Funding capture
Spot long against perpetual short collects funding with no repeated crossing cost after entry.
Small-notional inefficiencies
Precisely the opportunities institutions ignore because they cannot move enough capital through them to matter.
What these share is that the edge is informational rather than latency-based. The version 8 architecture optimised for microseconds; at this capital level the binding constraint is information and fee-to-edge ratio. Version 9 adds the machinery for the constraint that actually binds.
2. The Constraints
2.1 Technology Rules
Rule
Scope
Excludes
Every application is written in Rust.
Hot path, warm services, batch jobs, solvers, training, ingestion, tooling, frontend.
Python, TypeScript, Java, Go, C++ as a primary language.
Every managed service is Google Cloud or IBM.
Anything paid for or depended on operationally at runtime.
Third-party SaaS, external observability, non-IBM quantum, relay networks.
The vendor rule constrains infrastructure, not counterparties. Exchanges, brokers, custodians, banks, chain endpoints, registries, and information sources are systems the platform trades against, holds capital with, or observes. They are not managed services in the sense the rule addresses.
2.2 Architectural Rules
Rule
Consequence
Belief precedes action.
Every decision carries a confidence, and position size scales with it. A system that cannot distinguish no signal from conflicting signals will size both identically.
Correlation is not carried into production alone.
A relationship used for sizing or allocation must have a causal hypothesis attached, even a weak one, so that a regime break can be reasoned about rather than merely survived.
Every path not taken is scored.
Vetoes, filtered opportunities and unfunded cycles are shadow-executed and evaluated. The gate is a learning instrument, not only a control.
The system models its own competence.
It knows where its estimates are unreliable and directs exploration there, rather than only learning from trades taken for other reasons.
Feasibility precedes profitability.
Can this execute at my size, given minimum order, fee floor, tick and gas, is asked before whether it is profitable.
Strategies are compiled, not interpreted.
One shared evaluation plan with common subexpressions eliminated. This is what makes tens of thousands tractable.
No strategy sends an order.
Strategies produce intents; intents are netted, gated, then executed. One net order, many contributors.
Risk reads aggregates, never strategy lists.
A risk check is O(1) in strategy count, always.
Match the mechanism to the span.
Eight arbitrage paths. Cross-region coordination happens in advance as policy, never at execution.
Capital moves autonomously, only inside signed corridors.
Humans approve where money can go. Machines decide when.
Quantum output is policy, never a live instruction.
Family allocations, whitelists, path assignments, inventory targets, exposure limits.
Promotion is statistical, not discretionary.
At ten thousand candidates human judgement cannot be the gate. Multiple-testing correction and live confirmation are.
No language model touches a trade, a cycle, or a transfer.
Models generate hypotheses and summarise. Deterministic code and formal optimisers decide.
After-tax return is the only return.
Lot selection, holding period and jurisdiction are part of the decision, not a reporting afterthought.
3. Design Principles
#
Principle
How It Shows Up
1
Understand before predicting.
A causal graph and a world model sit upstream of every statistical model. Prediction without understanding fails silently at exactly the moment it matters.
2
Remember, do not only average.
Streaming estimators forget by design. Episodic memory exists so the system can recognise a situation rather than merely reflect a moving average of it.
3
Learn from what you did not do.
The richest signal in a trading system is the distribution of paths declined. Scoring them costs almost nothing and is discarded by almost everyone.
4
Know the shape of your ignorance.
A self-model that maps where estimates are unreliable is what turns exploration from random into directed.
5
Scale by indexing, never by iteration.
Strategies, cycles, entities, episodes and risk all use the same pattern: precompute an index, wake only what a change touches.
6
Latency is physics. Design around it.
Cross-region atomicity is impossible, so four mechanisms exist that do not need it.
7
Determinism where money moves.
Risk gating, netting, transfer gating, accounting and settlement are ordinary code with exhaustive fixtures.
8
Statistical honesty is a first-class control.
At ten thousand strategies an uncorrected backtest is a random number generator with a Sharpe ratio attached.
9
Own no data you can fetch.
Data gravity is an anchor. The platform holds statistics, episodes and its own irreplaceable records, and points at everything else.
10
Degrade, do not fail.
Every dependency loss has a defined reduced-capability mode, ending in flat and halted.
11
Explain, not just attribute.
Attribution says which strategy caused a fill. Explanation says what the system believed and why. Only the second earns trust.
12
Every component earns its place.
If removing it does not break a stated requirement, it is removed.
4. Architecture Overview
   INGESTION            news · filings · releases · chains · registries · physical
      |                 prices · order books · own outcomes
      v
   COGNITION            world model  ->  causal graph  ->  belief state
      |                 episodic memory  ·  counterfactuals  ·  self-model
      |                 hypotheses
      +--------------------------+
      v                          v
   VALUATION                 INTELLIGENCE
   term structure · credit    train models · generate and gate strategies
   vol surface · illiquid     risk and corridor policy
   cashflow · corp actions        |
      |                          v
      +------------------->  OPTIMISATION
                             regime-conditional allocation · cycle selection
                             path assignment · capital placement · multi-horizon
                                 |
                  SHIPPED: models · beliefs · episodes · strategies
                           grants · limits · targets · feasibility
                                 v
   +-------------+-------------+-------------+
   |  EXECUTION  |  EXECUTION  |  EXECUTION  |   regional nodes
   |  Americas   |   Europe    | Asia-Pacific|   microseconds
   +------+------+------+------+------+------+
          |             |             |
       venues        venues        venues
          +-------------+-------------+
                        v
        LEDGER · WALLET · TREASURY      per user, per strategy
                        |
                        +------------> back to INGESTION and COGNITION
The loop is no longer train, ship, execute, retrain. It is observe, understand, believe, value, allocate, act, remember. Every outcome re-enters the system in three places at once: the ledger as a fact, cognition as an episode and a counterfactual, and intelligence as training signal.
4.1 Why Cognition Is Its Own Plane
It has a different object of study. Every other plane reasons about instruments, prices and capital. Cognition reasons about the world that produces them, and about the platform itself. Folding it into Intelligence would bury the distinction that matters most — that a statistical model predicts, while a causal model explains, and only the second survives a regime it has never seen.
It also has a different failure mode. If Cognition degrades, the platform does not stop; it becomes less certain, sizes smaller, restricts itself to strategies whose edge does not depend on world state, and says so. That is a graceful degradation no other plane provides.
4.2 What Exists Once Versus Per Region
Global — exists once
Regional — exists per region
Ingestion and entity resolution
Executable graph and cycle scanner
World model and causal graph
Compiled strategy plan
Episodic memory, full index
Episodic memory, compact shipped index
Belief state, authoritative
Belief priors, cached with TTL
Counterfactual scoring
Feasibility gate
Self-model and exploration budget
Cycle-level risk gate
Valuation engines
Inventory manager and reservation table
Model training and registry
Inference, tract, in process
Quantum and classical optimisers
Leg coordinator and execution
Ledger, wallet, treasury
Local position, mirror and settlement cache
5. Domains by Plane
Thirty-four bounded domains across seven planes. Each owns its data and exposes a narrow interface.
5.1 Ingestion
#
Domain
Owns
New
1
Market Connectivity
Venue sessions, RFQ, quote streams, per-venue latency measurement.
—
2
Information Ingestion
News, filings, economic releases, chain events, registries, physical indices, weather, shipping.
NEW
3
Entity Resolution
Mapping every observation to a stable entity: company, person, country, commodity, contract, event.
NEW
3a
Source Discovery
Crawls surface and dark web to find and register sources and create feeds. Never ingests content directly. Hard exclusion on non-public information.
NEW
5.2 Cognition
#
Domain
Owns
New
4
World Model
Entities, relationships, supply chains, ownership, dependency. The semantic substrate.
NEW
5
Causal Inference
Causal graph over drivers, edge strength and evidence, interventional reasoning.
NEW
6
Episodic Memory
Compressed state vectors with outcomes, indexed for analogical retrieval.
NEW
7
Belief State
Probability distributions over world states, Bayesian updating, confidence propagation.
NEW
8
Counterfactual Learning
Shadow execution and scoring of every path declined, vetoed or unfunded.
NEW
9
Self-Model
Capability and uncertainty estimation. Where the platform is reliable and where it is guessing.
NEW
10
Hypothesis Generation
Proposed relationships, strategy classes and market structures, statistically gated.
NEW
5.3 Valuation
#
Domain
Owns
New
11
Term Structure
Yield curves, discount factors, forwards, convexity, roll.
NEW
12
Credit
Default probability, recovery, spread decomposition, covenant tracking.
NEW
13
Volatility Surface
Surface construction, skew, term structure, dispersion.
NEW
14
Illiquid Valuation
Marks without a continuous price: comparables, DCF, model, last round.
NEW
15
Cashflow and Commitments
Irregular contingent streams, capital calls, drawdown scheduling, J-curve.
NEW
16
Corporate Actions
Splits, dividends, mergers, spinoffs, rights. Without it equities silently break.
NEW
5.4 Intelligence
#
Domain
Owns
New
17
Strategy Lifecycle
Generation, backtest, statistical validation, promotion, capacity, decay, retirement.
—
18
AI/ML Platform
Online and batch training, registry, evaluation, drift, ONNX export.
—
19
Meta-Learning
Which model class works in which regime. Transfer across venues and asset classes.
NEW
20
Adversarial Modelling
Who is on the other side, how they adapt, whether the platform&apos;s own pattern is being learned.
NEW
21
Market Simulation
Adaptive counterparties that respond to the platform&apos;s own orders.
NEW
22
Risk Policy
The exposure envelope at ten levels, crowding response, breadth floor.
—
5.5 Optimisation
#
Domain
Owns
New
23
Allocation
Family, regime-conditional and multi-horizon allocation. Cycle selection and path assignment.
extended
24
Capital Engine
Deployed share against reserve, exploration budget, transfer sizing and routing.
extended
25
Scenario and Stress
Shock propagation through the causal graph rather than historical replay.
NEW
5.6 Execution
#
Domain
Owns
New
26
Market Intelligence
Normalisation, book state, feature computation and dependency indexing.
—
27
Strategy Engine
Compiled plan, evaluation tiers, subscription index, intent generation.
—
28
Feasibility
Can this execute at my size given minimum, fee floor, tick and gas.
NEW
29
Market Making and Creation
Two-sided quoting, inventory skew, and origination where no market exists.
extended
30
Arbitrage
Executable graph, cycle detection, path routing, leg coordination.
—
31
Intent Netting
Aggregation, internal crossing, contributor attribution, self-trade prevention.
—
32
Inventory and Mirrors
Stages, settlement timeline, reservation, cross-region mirrors and direction gating.
—
32a
Registries
Asset class registry, venue onboarding, settlement calendars, hedge map, cross-margin model. What may be traded, where, and under what conventions.
NEW
32b
Position Lifecycle
Divestment as a first-class flow: orphaned positions, unwind policy, thesis expiry, ordering against the liquidity ladder.
NEW
5.7 Ledger, Experience and Platform
#
Domain
Owns
New
33
Ledger, Wallet and Treasury
Money state per user and per strategy, reconciliation, corridors, custody, tax lots.
extended
34
Experience and Identity
Web and mobile, portfolio, wallet, strategy selection, explanation, authentication, mandates.
extended
Observability, Data Policy and Security are cross-cutting rather than bounded domains and appear in every plane. They are specified in Sections 44 through 48.
6. The Cognitive Loop
Seven stages, each feeding the next, and every outcome re-entering at three points. This is the organising structure of the whole platform.
Stage
What happens
Produces
Cadence
Observe
Prices, books, news, filings, releases, chain events, registries, physical indices, and the platform&apos;s own fills and verdicts arrive and are resolved to entities.
Events linked to entities
continuous
Understand
Events update the world model. The causal graph is re-estimated. New relationships are proposed as hypotheses and tested.
Causal edges with evidence
minutes to daily
Remember
The current state is compressed to a vector, stored with what followed, and indexed so a similar state can be retrieved later.
Episodes
continuous
Believe
Evidence is combined into explicit probability distributions over world states, with confidence that propagates into sizing.
Beliefs with confidence
seconds to minutes
Value
Assets without a continuous price are marked: curves, credit, surfaces, comparables, discounted cashflows, commitments.
Valuations with method and confidence
minutes to daily
Allocate
Capital is split across families, regimes and horizons under the risk envelope, with an explicit share reserved for information gain.
Grants, whitelists, targets
adaptive
Act
Regional nodes combine shipped intelligence with live local state, check feasibility, gate, and execute.
Orders, quotes, transfers
microseconds
Learn
Outcomes and the paths declined are both scored. The self-model updates. Exploration is redirected toward where confidence is weakest.
Counterfactuals, capability estimates
continuous
6.1 The Three Return Paths
   an outcome re-enters the system three times:
     -> LEDGER        as a fact          per user, per strategy, immutable
     -> COGNITION     as an episode      what the world looked like, what followed
                      as a counterfactual  what the alternatives would have yielded
     -> INTELLIGENCE  as training signal   labelled example for the next model
Version 8.0 had only the first and third. The second is what makes the loop cognitive: without episodes the platform cannot recognise, and without counterfactuals it can only learn from choices it happened to make.
6.2 Degradation Order
Each cognitive capability has a defined behaviour when it is unavailable or stale, and the platform narrows rather than halting.
Unavailable
Behaviour
Ingestion stalls
World model ages. Event-driven and prediction-market strategies pause. Price-only strategies continue unaffected.
Causal graph stale
Regime-conditional allocation reverts to unconditional. Sizing becomes more conservative because relationships can no longer be reasoned about.
Episodic memory unavailable
Analogical retrieval unavailable. Strategies depending on situational recognition pause; the rest continue.
Belief state stale beyond TTL
Confidence-weighted sizing falls back to a fixed conservative multiplier. Nothing halts.
Counterfactual scoring down
Learning slows. No trading impact whatsoever — it is entirely a warm-path function.
Self-model stale
Exploration budget reverts to a flat allocation instead of a directed one.
Valuation engine down
Assets without a continuous price are frozen at last mark and flagged. Continuously priced assets are unaffected.
7. Information Ingestion
Version 8.0 observed prices. That is enough for arbitrage and microstructure and nothing else. Prediction markets settle on real-world outcomes, event-driven strategies need events, private assets need filings and comparables, and physical arbitrage needs shipping costs. All of it starts here.
7.1 Sources
Class
Sources
Cadence
Unlocks
Market
Venue books, trades, quotes, funding, RFQ
microseconds
Everything already built
Corporate
Filings, earnings, guidance, insider transactions, corporate actions
minutes to daily
Equities, credit, event-driven
Economic
Central bank decisions, inflation, employment, PMI, auctions
scheduled
Rates, FX, macro, prediction
News and text
Wire services, press releases, regulatory notices
seconds to minutes
Event-driven, prediction, sentiment
On-chain
Transfers, contract state, governance, liquidity, bridges
block time
DeFi, wrapped assets, chain arbitrage
Registry
Carbon, renewable certificates, provenance, title, patents
daily
Environmental, collectibles, IP
Physical
Shipping rates, port congestion, weather, satellite, inventories
hourly to daily
Commodities, freight, product arbitrage
Resolution
The authority that settles a prediction market or event contract
on resolution
Prediction markets, sports, insurance-linked
Own outcomes
Fills, verdicts, quotes, transfers, reconciliations
continuous
Every learning loop
7.2 Ingestion Is Also Pass-Through
The data policy applies here without exception. Information is not archived; it is resolved to entities, folded into the world model and the causal graph, retained as an episode where it mattered, and otherwise discarded. What persists is meaning, not text.
Retained
Discarded
Entity references and their relationships
Raw article and filing text
Extracted facts with source and timestamp
The document that carried them
Causal edges the evidence supported
Evidence that supported nothing
Episodes where the event preceded a material outcome
Events that preceded nothing
A manifest pointing at the original, with a content hash
A copy of the original
A filing is a few hundred kilobytes. The facts it establishes are a few hundred bytes. Storing the second and pointing at the first is the same discipline the platform already applies to market data, extended to text.
7.3 Ingestion Pipeline
Stage
Work
Fetch
Poll or stream from the source. Rate-limited, backed off, availability-scored like any venue adapter.
Deduplicate
The same event arrives from several sources. Content hashing and near-duplicate detection collapse it to one.
Extract
Structured facts from semi-structured text. Numbers, dates, parties, amounts, actions.
Resolve
Every party, instrument and place mapped to a stable entity identifier. This is the hard part and Section 8 covers it.
Timestamp
Both event time and receipt time recorded separately. The gap is itself a signal about how fast a source is.
Assess
Source reliability, historical accuracy, and whether this source has been early or late before.
Emit
A WorldEvent linked to entities, with confidence, published to Cognition.
Language models assist extraction and resolution and have no authority over anything downstream. An extracted fact carries a confidence and a provenance, and a low-confidence extraction updates beliefs weakly rather than being treated as established.
7.4 Source Discovery — Finding New Feeds
Ingestion so far consumes known sources. A cognitive platform should also find sources it does not yet know about. The discovery layer crawls three tiers of the web, each with a different access model and a different role. It evaluates whether a location is a useful, lawful, recurring source and, if so, registers it and creates a feed. Everything found then flows through the ordinary pipeline in 7.3.
Tier
What it is
Role
Feeds training?
Surface web
Indexed by search engines. News, exchange status pages, project sites, public forums
Discovery and feeds
Yes
Deep web
Not indexed. Database-driven portals, paywalled and licensed content, login-gated archives, records systems. The overwhelming majority of the web, and where the underused information lives
Discovery and feeds. The largest legitimate breadth gain available
Yes — see 7.5
Dark web
Tor and I2P hidden services
Defensive monitoring only. Registers threat indicators, never content
No — see 7.6
   CRAWL       breadth-first over seeds and links, surface and dark web
     v
   ASSESS      is this a useful, lawful, recurring source of signal?
     v            (registers the source, NOT the content)
   REGISTER    create a DataReference or a feed definition with a schema
     v
   FEED        the source now flows through the ordinary 7.3 pipeline
     v
   SCORE       reliability, timeliness, uniqueness tracked over time
The distinction between registering a source and ingesting content is the whole design. The crawler is a scout, not a hoarder. It samples enough of a location to decide whether it is worth a feed, records how to reach it and what shape its data takes, and moves on. The pass-through data policy applies without exception: the platform ends up with a catalogue of sources and a set of feeds, not a copy of the web.
Stage
What it does
What it produces
Seed and expand
Start from known-good sources, follow links, and expand into related domains
Candidate locations
Classify
Is this news, filings, data, discussion, a marketplace, a leak forum?
A source category
Sample
Read enough to judge usefulness, recency and structure. Never a full crawl
A quality and structure estimate
Lawfulness and policy check
Screen against the exclusions in 7.5 before anything is registered
Registered, or rejected with a reason
Register
Create a DataReference or a feed with source, endpoint, schema, cadence and access method
A source, not content
Hand off
The registered source enters the 7.3 pipeline like any other
A live feed
Score over time
Reliability, timeliness, uniqueness, and whether it has ever been early
An availability and value score
7.5 The Dark Web, and the Hard Line
Dark web monitoring is a legitimate and common activity for security and threat intelligence. Firms watch it to learn that their own credentials have leaked, that a venue they use has been breached, or that infrastructure they depend on is being targeted. That is defensive, and it is the only reason Algorik crawls it.
There is a bright line, and it is a legal one rather than a technical one. Trading on material non-public information is insider trading regardless of where the information was found. A leaked earnings figure discovered on a forum is exactly as illegal to trade on as one whispered in a boardroom. The discovery layer is built so that crossing this line is not possible through it, not merely discouraged.
Permitted — defensive and lawful
Excluded — hard, at the source
The platform&apos;s own credentials or keys appearing in a dump
Any material non-public information about a tradeable entity
A venue or custodian the platform uses being breached or targeted
Stolen data, leaked documents, or anything offered for sale illicitly
Infrastructure or a dependency being discussed as a target
Personal data on private individuals
Sentiment and narrative that is already public, aggregated
Anything whose use would constitute insider trading, market manipulation or receipt of stolen property
Threat indicators feeding the anomaly detector and the transfer gate
Content that requires payment, intrusion, or credentials to obtain
The exclusions are enforced at classification, before registration, and a candidate that trips any of them is rejected with a recorded reason rather than registered and filtered later. A source is registered only if its information is public, lawful to use, and free of material non-public content. Where a jurisdiction or the operator&apos;s compliance posture is stricter than this line, the stricter rule governs — the exclusions are a floor, not a ceiling.
Two operational safeguards follow from the legal one. First, dark web access is isolated in its own hardened enclave with no path to any capital-moving component, because it is the single most hostile network the platform touches. Second, everything the discovery layer registers is auditable — what was crawled, what was rejected and why, and what became a feed — so the defensive purpose is demonstrable rather than asserted.
7.6 Deep Web Ingestion — Where the Edge Actually Lives
The deep web is not the dark web. It is the unindexed majority of the internet: content behind query forms, dynamic rendering, free registrations, and licensed subscriptions that search engines never surface. Regulatory databases, customs records, court dockets, procurement portals, shipping trackers, job boards, governance forums, pricing and inventory pages. All of it lawful and public, almost none of it read by the people who move markets, because they read the surface web and each other.
This is the informational edge the platform is built to capture, and it is the arena where small capital wins. A hedge fund cannot deploy meaningful size on the basis of a customs filing or a procurement notice. Algorik can, and the deep web is where those signals appear first.
7.6.1 Source Categories
Category
Examples
Signal it carries
Regulatory and legal
Full-text filing search, company registers, transparency registers, broker records, court dockets, patent and trademark offices
Corporate events before they are news. Litigation exposure. Ownership changes. Innovation pipelines
Government and trade
Customs and trade data, procurement portals, permit filings, drug and device approvals, energy and utility regulators, statistical agencies
Real activity before it reaches an income statement. Supply chain movement. Regulatory outcomes
Corporate self-disclosure
Investor relations, product and pricing pages, job postings, press rooms, developer changelogs, status pages
Hiring velocity, price changes, product launches, outages — all public, all leading
Physical and geospatial
Vessel tracking, port authorities, satellite imagery services, commodity inventory reports, weather services
Physical flow that precedes financial flow. Commodity supply. Freight congestion
Community and technical
Developer forums, repository activity, governance forums for on-chain projects, specialist trade forums
Sentiment and intent that is public but unaggregated. Protocol changes before they ship
Academic
Pre-print servers, working paper series, conference proceedings
Methods and findings before they are commercialised
Marketplace
Product listings, price histories, inventory levels, auction results
Direct input to product arbitrage and to consumer demand signals
Resolution sources
The official pages that determine event outcomes
What prediction markets settle on. Knowing the source is knowing the answer&apos;s timing
7.6.2 Access Modes
Deep web sources are not reachable by following links. Each needs an access adapter, and the mode determines what the adapter must do and what policy governs it.
Mode
How
Policy
Open query
Parameterised requests to a form or search interface with no login
Respect robots.txt and rate limits. Enumerate parameters, do not hammer
Structured API
A published API exists beside the page
Always preferred over scraping. Registered as a feed directly
Free registration
A legitimate account is created and used
One account per source, credentials in Secret Manager, terms of service honoured
Licensed subscription
Paid access to a data product
Licensed in the operator&apos;s name. Redistribution terms recorded on the source
Rendered page
Content produced by client-side code
Headless rendering in the discovery enclave, at the source&apos;s rate limit
Bulk download
Periodic full extracts offered by the source
Fetched into the research cache, facts extracted, extract deleted per the data policy
Three things are never done regardless of mode: circumventing a paywall or access control, sharing or automating credentials beyond the source&apos;s terms, and collecting personal data on private individuals. The deep web is lawful because these lines are kept, and a source that requires crossing one is rejected at classification rather than registered.
7.6.3 The Adapter Pattern
   DeepWebAdapter {
     source          the registered SourceCandidate
     mode            open_query | api | registered | licensed | rendered | bulk
     access          credentials reference, rate limit, ToS record
     query_plan      what to ask, how often, what parameters to enumerate
     extractor       schema for turning the page or response into facts
     entity_links    which entity types this source speaks about
     freshness       how far ahead of the surface web this source runs
   }
An adapter is small — a query plan and an extraction schema — because the pipeline in 7.3 does everything after extraction. Adding the hundredth deep web source is a repeat of adding the first. The discovery crawler proposes adapters from what it finds; a human approves the source category once; adapters within an approved category are promoted automatically as their extraction accuracy is measured.
7.6.4 How It Feeds Training
Deep web facts enter training exactly as any other fact does: through the world model, the causal graph, beliefs and episodes, never as raw content. What the models learn from is meaning, resolved to entities and timestamped.
Deep web fact
Becomes
Trains
A customs filing showing a component shipment
A WorldEvent linked to supplier and manufacturer entities
The causal graph — supply drives production drives revenue
A hiring surge on a company&apos;s job board
An attribute change on the entity, timestamped
Regime and event-driven models; episodes where hiring preceded a result
A procurement award to a listed contractor
A WorldEvent with amount and counterparty
Belief formation for that entity; event-driven strategies
A governance proposal on a protocol forum
A WorldEvent with a resolution source and date
Prediction-market resolution and calibration
Vessel congestion at a commodity port
A physical-index observation
Carry and basis models; causal edges from freight to commodity
A price change on a product page
A marketplace observation
Product arbitrage feasibility; consumer demand signals
The freshness field is the one that matters most. A source that runs ahead of the surface web by hours is worth more than one that runs ahead by minutes, and a source that is merely a copy of what is already public carries no edge at all. Freshness is measured, not assumed — the platform records how far in advance each source&apos;s facts preceded the same fact appearing elsewhere, and sources are ranked on it.
7.6.5 Why This Is the Right Kind of Edge
Property
Why it matters
Lawful
Every source is public, licensed, or a legitimate free registration. Nothing is circumvented. The advantage survives scrutiny
Durable
It does not decay under competition the way a latency edge does. Most participants will keep reading the surface web
Small-capital compatible
The signals are informational, and the opportunities they surface are often too small for institutions to bother with
Compounding
Every registered source improves the world model, which improves every strategy that reads it. The catalogue is an asset
Explainable
A position built on a customs filing has a plain-language explanation. A position built on a latency race does not
7.6.6 Deep Web Governance
Rule
Reason
Every source carries a terms-of-use status: public, licensed, or restricted
A source whose terms forbid automated access is not registered. Getting blocked or sued for aggressive collection is an operational risk, not a hypothetical
Politeness is enforced: robots directives, rate limits, back-off, an identified user agent
A well-behaved crawler keeps its access. An aggressive one loses it for everyone
Licensed sources are used within licence, with credentials scoped per source in Secret Manager
A licence violation is a breach of contract and forfeits the source
The exclusions in 7.5 apply without change
A deep web source that surfaces material non-public information is excluded exactly as a dark web one would be. Where it was found does not change what it is
Registration is not ingestion
The crawler samples enough to register a feed and its schema. The feed then flows through 7.3 and pass-through applies
Personal data on private individuals is not registered
It is not a trading signal, and holding it is a liability
The distinction between deep and dark is one of access, not legality, and the governance reflects it. The deep web is crawled broadly and used fully, within terms. The dark web is crawled narrowly and used defensively. Both feed the same pipeline, and the same exclusions sit at the front of it.
8. The World Model
A semantic substrate: what exists, how it relates, and what has happened to it. Without this there is no way to connect a filing to an instrument, a shipping delay to a commodity, or a court ruling to a prediction contract.
8.1 What It Holds
Object
Examples
Why it matters
Entity
Company, person, sovereign, commodity, contract, venue, chain, index, event
The anchor everything resolves to. Without stable identity, nothing connects
Relation
Owns, supplies, competes, depends on, is domiciled in, is collateral for, settles
Turns a list of entities into a graph that can propagate a shock
Instrument link
This entity is represented by these instruments across these venues
The bridge from the world to the executable graph
Event
A filing, release, ruling, delivery, hack, election, weather outcome
What changes state, and what a prediction market resolves on
Attribute
Sector, jurisdiction, credit rating, float, supply schedule, expiry
Conditions and filters for strategy universes
Resolution source
The authority that determines a contract&apos;s outcome
Prediction markets are unsafe without knowing who decides and when
8.2 Why the Graph Structure Earns Its Place
A supply disruption at one company is a fact. That the same company supplies four listed manufacturers, two of which have prediction contracts on delivery timing, and that a commodity it consumes has a futures curve in contango, is a chain of reasoning. Only a graph can traverse it, and only traversal turns one observation into several positions.
Query the graph enables
Strategy it serves
Which instruments are exposed to this entity, directly or through two hops?
Event-driven propagation
Which prediction contracts resolve on an event this entity controls?
Prediction market positioning
What is the shortest causal path from this macro release to this instrument?
Macro-conditional sizing
Which of my current positions share an underlying exposure I have not counted?
Hidden concentration detection
Which entities does my portfolio depend on that I do not hold?
Second-order risk
8.3 Size and Storage
A world model covering the entities Algorik plausibly touches — tens of thousands of companies, sovereigns, commodities, contracts and venues, with their relationships and a rolling window of events — is a graph in the low gigabytes. It lives in Spanner as node and edge tables, is queried by the cognition services, and is shipped to regions only as a compact digest of the relationships that currently matter.
Entity resolution is the part that will consume the most effort and it is worth naming plainly. The same company appears under a ticker, a legal name, a registry identifier and a colloquial name, and getting that wrong quietly corrupts everything downstream. Resolution confidence is recorded per link and low-confidence links are excluded from sizing decisions.
9. Causal Inference
The largest gap in version 8.0. Every model there was association: this feature correlates with that outcome. The consequence is specific and severe — when a regime breaks, models that learned the same spurious structure break together, and nothing in the system can say which relationships should have survived.
9.1 What Is Modelled
Layer
Contents
Estimated from
Drivers
Rates, funding, liquidity, flow, breadth, volatility, macro surprise, supply and demand shocks
Ingestion and market data
Mechanisms
How a driver propagates: through cost of carry, through collateral, through sentiment, through physical constraint
Hypothesis plus evidence
Edges
A directed relationship with strength, lag, sign, and the evidence supporting it
Observational plus natural experiments
Conditions
The regime under which an edge holds, and the conditions under which it is known to fail
Historical regime segmentation
Confounders
Common causes that would otherwise produce a spurious edge
Explicit, and adjusted for
9.2 How Edges Are Established
Method
Use
Strength
Natural experiments
Scheduled releases, expiries, index rebalances and hard forks act as interventions the platform did not have to cause
Strongest available evidence outside a real experiment
Instrumental variables
A driver that affects the outcome only through the proposed mechanism
Strong where a valid instrument exists
Granger-style lead-lag with controls
Temporal precedence with confounders explicitly adjusted
Weak alone, useful as a filter
Structural constraints
Arbitrage relationships and accounting identities that must hold
Certain, and the anchor the rest is calibrated against
The platform&apos;s own trades
Its own order flow is a small intervention with a known cause
Directly interventional, and unique to the platform
Hypothesis plus falsification
A proposed mechanism, a prediction it implies, and a test that could refute it
Weak until tested, and the only source of genuinely new edges
The platform&apos;s own order flow deserves emphasis. It is the one intervention Algorik controls, with a known cause and a measurable effect, and it is the cleanest causal evidence available anywhere in the system. Every fill is a small experiment.
9.3 What the Causal Graph Is Used For
Use
Effect
Regime-break survivability
Edges tagged with the conditions under which they hold. When conditions change, strategies depending on failing edges are reduced before their performance degrades, rather than after.
Shock propagation
A stress scenario propagates through mechanisms rather than through historical correlation, so it can model a shock the history does not contain.
Hidden concentration
Positions that appear diversified but share a causal driver are surfaced as concentration.
Feature validation
A feature with predictive power and no causal path is flagged as likely spurious and its allocation is capped.
Explanation
Why the platform believes what it believes, expressible as a path through the graph rather than as a model weight.
9.4 Honest Limits
Limit
Handling
Causal discovery from observational data is unreliable
Edges carry a confidence. Low-confidence edges inform exploration, not sizing.
Financial markets are non-stationary
Edges are conditioned on regime and re-estimated. An edge that fails its conditions is retired, not patched.
Confounders are often unobserved
Where a plausible unobserved confounder exists it is recorded as such, and the edge is treated as suggestive rather than established.
The graph will be wrong in places
Which is why it constrains sizing and explanation rather than generating trades directly. A wrong edge produces a suboptimal allocation, never an unbounded position.
10. Episodic Memory
Streaming estimators forget by construction. An exponentially weighted covariance is a moving average of the recent past, and a reservoir sample is a thin cross-section. Neither can answer the question a human trader answers instinctively: have I seen this before, and what happened?
10.1 What an Episode Is
   Episode {
     state_vector     compressed market and world state at time t
     regime           the regime label in force
     causal_context   which edges were active and their strength
     beliefs          what the platform believed, with confidence
     actions          what it did, and what it declined
     outcome          what followed, over several horizons
     surprise         how far the outcome was from what was expected
   }
Episodes are not raw ticks. A state vector is a few hundred numbers, an outcome is a few dozen. Years of episodes at a meaningful sampling density is single-digit gigabytes, which sits comfortably inside the data policy — this is compressed meaning, not retained observation.
10.2 What Gets Stored
Trigger
Rationale
Every material action
Fills, quotes, cycles, transfers. The platform&apos;s own behaviour is irreplaceable.
Every high-surprise moment
Where the outcome diverged most from expectation. These are the most informative and the rarest.
Every regime transition
The boundaries are where models fail, and where the platform most needs recall.
Periodic sampling in calm
Sparse baseline so that normality is represented and not only crises.
Every veto and near-miss
What the platform nearly did is as informative as what it did.
10.3 Retrieval and Use
Query
How it is served
Feeds
What does now most resemble?
Nearest neighbours in state-vector space, weighted by regime and causal context
Sizing, strategy activation
What followed those situations?
Outcome distribution across retrieved episodes
Belief formation, expected value
Has this strategy faced this before?
Episodes filtered to those where the strategy was active
Strategy-level confidence
Was this surprising last time too?
Surprise scores across analogues
Self-model, exploration targeting
What did we decline in situations like this, and should we have?
Joins episodes to counterfactual scores
Gate calibration
Retrieval is approximate nearest neighbour over a few hundred dimensions across a few million episodes. That is milliseconds on a warm-path service, so it informs belief and sizing rather than sitting in the microsecond path. Regions receive a compact digest — the outcome distribution for the current neighbourhood — rather than the index itself.
11. Belief State and Uncertainty
Version 8.0 produced point estimates. A slippage number, a regime label, a pressure score. Nothing in the system could distinguish a confident estimate from a coin flip, which meant both were sized identically.
11.1 What a Belief Is
   Belief {
     proposition    a statement about the world that could be true or false
     distribution   a probability distribution, not a point
     evidence       what updated it, with source and weight
     causal_path    why this evidence bears on this proposition
     confidence     how much the distribution should be trusted
     ttl            when it becomes stale absent new evidence
   }
Proposition class
Example
Updated by
Regime
The market is in a high-volatility mean-reverting regime
Market state, causal conditions, episodic analogues
Event outcome
This contract resolves yes
News, filings, base rates, resolution source
Relationship
This edge is currently active
Natural experiments, own flow, lead-lag with controls
Counterparty
This venue honours firm quotes at this rate
Observed honour outcomes, conjugate update
Capacity
This strategy&apos;s edge survives at three times current size
Own probing trades, impact modelling
Valuation
This illiquid asset is worth within this range
Comparables, discounted cashflow, last round
11.2 Confidence Drives Size
This is the operational consequence and it is the point of the whole section. Position size is a function of edge, volatility, capital grant and now confidence. A high-edge low-confidence signal and a moderate-edge high-confidence signal are no longer indistinguishable.
Belief state
Sizing response
High confidence, strong edge
Full size within the grant
High confidence, weak edge
Small size, taken because it is reliable
Low confidence, strong edge
Reduced size, and flagged to the exploration budget as worth learning about
Low confidence, weak edge
Not taken. Feasibility and profitability filters would likely reject it anyway
Conflicting evidence
Distribution is wide. Size scales down with its width, and the conflict itself is recorded as an episode
No evidence
Distinguished explicitly from conflicting evidence. Prior only, minimum size or abstain
Distinguishing an absence of evidence from a conflict of evidence is one of the sharpest practical differences between a cognitive system and a statistical one. Both look like a middling score to a point estimator, and they call for opposite responses: abstain in the first case, investigate in the second.
11.3 Propagation to Regions
Beliefs are formed centrally, where the evidence is, and shipped as priors with a time-to-live. A region applies the prior to its sizing and never forms a belief of its own — it does not have the world model to do so. If the prior goes stale past its TTL, sizing falls back to a fixed conservative multiplier and the region reports that it is operating without current belief.
12. Counterfactual Learning
The cheapest large improvement available to this platform. The risk gate already logs every veto, including silences. Nothing scores them. If four hundred cycles are vetoed in a day and three hundred and eighty would have been profitable, that is an enormous signal being discarded daily.
12.1 What Gets Shadow-Executed
Path not taken
Why it is informative
Vetoed by the risk gate
Tells you whether the constraint is protective or merely expensive. This distinction is otherwise unknowable
Filtered by profitability
Tells you whether the fee and slippage model is too pessimistic
Rejected by feasibility
Tells you which size thresholds are actually costing opportunity
Not whitelisted by the optimiser
Tells you whether the allocation was right, measured in realised profit rather than objective score
Alternative path for a cycle
Tells you whether path assignment is choosing well among eligible mechanisms
Alternative sizing
Tells you where the sizing function is systematically wrong
The strategy that did not fire
Tells you where a predicate threshold is mis-calibrated
12.2 How It Is Scored
   for each declined path:
     reconstruct   the book state at that instant, from event-anchored snapshots
     simulate      the fill, using the venue&apos;s own observed fill behaviour
     apply         realistic fees, slippage, and the impact the order would have had
     evaluate      the outcome over the horizon the strategy intended
     attribute     to the specific rule that declined it
     accumulate    per rule, per venue, per regime, per strategy
Event-anchored snapshots already capture book state at every veto, which was designed for execution quality analysis and turns out to be exactly what counterfactual reconstruction needs. The capability comes almost free because the data is already being kept.
12.3 What It Changes
Finding
Response
A rule vetoes mostly profitable paths
The rule is too tight. Recalibrate with evidence rather than intuition
A rule vetoes mostly losing paths
The rule is earning its place. Quantify by how much and defend it
A rule almost never fires
Either the risk never occurs or the rule is mis-specified. Investigate rather than leave it
Feasibility rejections cluster on one venue
That venue&apos;s minimum size or fee structure is the binding constraint. Consider dropping it
An unfunded family outperforms funded ones
The allocator&apos;s objective is mis-specified. This is the highest-value finding available
Alternative sizing consistently better
The sizing function needs work, and the counterfactual says exactly where
Every one of these is invisible to a system that only observes what it did. A platform that acts on ten thousand strategies and learns from only the fraction it executed is learning from a heavily selected sample, and selection bias in a learning loop compounds.
12.4 Guardrails
Risk
Control
Simulated fills flatter reality
Fill simulation is calibrated against actual fills on the same venue, and its error is tracked as a first-class metric
Impact is underestimated
Counterfactual size is capped at what observed depth would have absorbed, and impact is charged
Overfitting to counterfactual results
Counterfactual findings enter the same statistical gate as any other hypothesis, with the same trial accounting
Relaxing a control because it looks expensive
A veto rule may only be loosened through the full approval path, never automatically from counterfactual evidence
13. Self-Model and Active Learning
A system that knows where it is unreliable can direct attention there. Without that, learning is passive — it happens only where trades were taken for other reasons, which is a biased and slow-moving sample.
13.1 What the Self-Model Tracks
Dimension
Question it answers
Estimator reliability
Where is my slippage or dispersion model accurate, and where does its error exceed tolerance?
Coverage
Which venue, instrument, regime and size combinations have I actually observed, and which am I extrapolating into?
Calibration
When I say seventy percent, does it happen seventy percent of the time?
Capacity
At what size does each strategy&apos;s edge decay, and how confident am I in that number?
Regime experience
How many distinct regimes have I traded through, and how recently?
Model age
How stale is each model relative to the regime it was fitted in?
Blind spots
Which parts of the state space have no episodes at all?
13.2 The Exploration Budget
An explicit share of capital allocated to information gain rather than expected return. It is a line item in the capital engine, not a side effect.
Probe
Learns
Cost
Trade a strategy at three times normal size, once
Where its capacity actually decays
Bounded slippage on one trade
Post a resting order on an unfamiliar venue
That venue&apos;s fill and adverse-selection behaviour
An option premium, priced
Take a small position where the model is uncertain
Whether uncertainty is irreducible or merely unobserved
Small expected loss
Trade a rarely-active strategy deliberately
Refreshes a stale model and a stale capacity estimate
Small
Enter a regime-boundary trade
How the causal edges behave at the transition
Moderate, and highly informative
Allocation rule
Detail
Budget size
A stated percentage of deployed capital, set in the user mandate. Small, explicit, and never implicit
Selection
Upper-confidence-bound or Thompson sampling over uncertainty, weighted by the value of resolving it
Bounds
Every probe carries a maximum loss. An exploration trade is never allowed to become a position
Accounting
Reported separately from return-seeking capital, so exploration cost is visible rather than mixed into performance
Value measurement
The reduction in uncertainty a probe produced is scored, so the budget itself is optimised over time
Reporting exploration separately matters more than it appears. Mixed into performance it looks like drag, and the instinct is to cut it. Reported separately it is visible as the cost of learning, and its return can be measured in the improvement of the estimates it bought.
14. Hypothesis Generation
Version 8.0 generated strategies by parameter sweep and family template. That produces variation, not novelty — ten thousand near-substitutes of a few real ideas, which is precisely why effective breadth sits in the low hundreds.
14.1 What a Hypothesis Is
   Hypothesis {
     proposition   a causal claim, e.g. X drives Y through mechanism M
     mechanism     why it would be true, stated explicitly
     prediction    something observable that follows if it is true
     falsifier     what would have to be observed to reject it
     evidence      accumulated for and against
     status        proposed | testing | supported | refuted | retired
   }
Requiring a falsifier is the discipline that separates this from a search over feature combinations. A hypothesis that cannot be refuted cannot be tested, and something that cannot be tested has no place sizing a position.
14.2 Sources
Source
Produces
Gaps in the causal graph
Two drivers that co-move with no known mechanism connecting them
High-surprise episodes
Situations where the outcome diverged most from expectation are where the model is most wrong and most improvable
Counterfactual anomalies
A rule that vetoes profitable paths implies a relationship the platform has not modelled
World model traversal
Structural exposures that exist in the entity graph but that no current strategy expresses
Language model proposal
Reading filings, research and news to propose mechanisms a statistical search would not reach
Cross-asset transfer
A mechanism established in one asset class, proposed in another where it has not been tested
The language model proposes and never decides. Its output is a hypothesis with a stated mechanism and falsifier, which then enters the identical statistical gate as any other candidate, with the same cumulative trial accounting. It shortens the search; it does not shorten the proof.
14.3 The Path from Hypothesis to Capital
Stage
Gate
Proposed
Has a mechanism and a falsifier, and is not a restatement of an existing hypothesis
Testing
Falsifier evaluated against held-out data. Counts against the family&apos;s cumulative trial budget
Supported
Survived the falsifier. Becomes a candidate causal edge with low initial confidence
Expressed
A strategy is constructed to express it, and enters the normal promotion pipeline
Funded
Only after holdout and live canary, exactly as any other strategy
Refuted or retired
Recorded permanently. A refuted hypothesis is valuable — it stops the same idea being re-proposed
15. Meta-Learning, Adversaries and Simulation
15.1 Meta-Learning
Learning which model works where, rather than only learning the models.
Capability
What it does
Effect
Model selection by regime
Learns which model class performs in which regime, from realised outcomes
Stops using a trending-market model in a mean-reverting one
Feature generality
Learns which features transfer across venues and which are venue-specific artefacts
Prevents a venue quirk being mistaken for a market truth
Warm starts
A new venue inherits priors from venues it structurally resembles rather than starting cold
Weeks of learning compressed into a prior
Cross-asset transfer
Microstructure learned in crypto informs equity microstructure, adjusted for structural difference
Fewer independent models, better calibrated
Hyperparameter learning
Learns good training configurations from the history of training runs
Reduces the trial budget consumed by tuning
15.2 Adversarial Modelling
Version 8.0 estimated adverse selection, which is the symptom. It did not model who was causing it or how they adapt.
Question
Why it matters
Who is on the other side of my fills?
Flow classification distinguishes informed from uninformed counterparties, and they warrant opposite responses
Is my pattern being learned?
If fills systematically precede adverse moves at a predictable interval, the platform has become predictable
Is a counterparty adapting to me?
Deteriorating fill quality on a specific venue against a stable market is a signal about a counterparty, not the market
Am I crowded with others running the same idea?
Correlated flow arriving simultaneously with the platform&apos;s own is the earliest crowding signal available
What does my order flow reveal?
Sizing, timing and venue choice all leak intent. The platform should know what it is telling the market
Response
Mechanism
Fingerprint randomisation
Jitter on non-urgent timing, randomised child sizing, venue rotation among near-equivalent options
Toxic-flow widening
Quote spreads widen against counterparties whose fills consistently precede adverse moves
Selective withdrawal
Pull from a venue where the platform is being systematically picked off, rather than pricing wider indefinitely
Crowding response
Family caps tighten automatically when realised correlation with external flow rises
Irrelevant at two hundred dollars and existential at scale. It is specified now because the instrumentation — recording who filled what and what followed — must exist from the beginning or the history required to build it will not be there.
15.3 Market Simulation
Recorded history cannot respond to you. That is the hard ceiling on learning execution tactics from backtests: every fill in a replay is free, and in reality it is not.
Agent type
Behaviour
What it teaches
Passive liquidity
Posts and refreshes, withdraws under stress
Realistic queue dynamics and fill probability
Informed flow
Trades ahead of moves the simulator knows are coming
Adverse selection, honestly
Momentum follower
Amplifies moves
Impact and reflexivity
Competing arbitrageur
Races for the same cycles
Crowding, and whether an edge survives competition
Market maker
Quotes and skews on inventory
How spreads respond to the platform&apos;s own flow
Used for
Detail
Execution tactic learning
Reinforcement learning against reactive counterparties rather than a passive tape
Crowding stress
What happens when three participants run the platform&apos;s strategy simultaneously
Impact calibration
How much the platform&apos;s own size moves a market of a given depth
Capacity discovery
At what size does an edge disappear when the market responds
Failure rehearsal
Venue outages, liquidity withdrawal, correlated stress — rehearsed rather than experienced
The simulator is calibrated against reality continuously: its predicted fills are compared to actual fills on the same venue, and divergence is tracked. A simulator that is not calibrated is a source of confident, expensive error.
16. The Valuation Plane
Roughly sixty percent of global investable wealth has no continuous price. Version 8.0 assumed a book for everything, which structurally excluded fixed income, credit, private markets, real assets, royalties, collectibles and most of what a wealth platform would actually hold.
16.1 Six Engines
Engine
Produces
Unlocks
Term structure
Yield curves, discount factors, forward rates, convexity, roll
Government and corporate bonds, swaps, futures fair value, any discounted cashflow
Credit
Default probability, recovery assumption, spread decomposition, covenant state
Corporate bonds, credit default swaps, private credit, distressed
Volatility surface
Full surface with skew and term structure, dispersion, forward volatility
Options beyond parity arbitrage, variance trading, structured payoffs
Illiquid valuation
A mark with a method and a confidence: comparables, discounted cashflow, model, last round
Private equity and venture, real estate, art, private credit, collectibles
Cashflow and commitments
Irregular contingent stream forecasts, capital call schedules, drawdown and J-curve modelling
Private funds, royalties, litigation finance, structured settlements
Corporate actions
Splits, dividends, mergers, spinoffs, rights, delistings
Equities, without which positions and cost basis silently corrupt
Corporate actions is the one that looks like plumbing and is not. Without it an equity position quietly becomes wrong after the first split, cost basis drifts, reconciliation halts, and every downstream number is contaminated. It is a prerequisite for equities, not an enhancement.
16.2 Valuation Carries Method and Confidence
   Valuation {
     asset          what is being marked
     value          the mark, or a range
     method         comparables | DCF | model | last_round | quoted | matrix
     inputs         what it was derived from, with their own confidence
     confidence     how much weight this mark should carry
     as_of          when, and how stale it is permitted to become
     next_review    when it must be refreshed
   }
A mark without a method is an assertion. Every valuation in the platform carries how it was arrived at, so a portfolio holding both a quoted equity and a venture position is not summing two numbers that mean different things without saying so.
16.3 Illiquid Valuation Methods
Method
When used
Confidence
Quoted
A continuous two-sided market exists
Highest
Matrix
Similar instruments quote, and this one is interpolated from them
High, for bonds
Comparables
Observable transactions in similar assets, adjusted
Moderate, and adjustment-sensitive
Discounted cashflow
Forecast cashflows discounted at a rate reflecting risk
Moderate, and assumption-sensitive
Model
A pricing model with observable inputs
Depends entirely on input quality
Last round
The most recent primary transaction price
Low, and decays quickly with age
Cost
Held at acquisition cost absent anything better
Lowest, and an admission of ignorance
Marks decay. A last-round valuation six months old carries less confidence than one six days old, and the platform reduces its weight rather than treating it as equally true. Confidence enters position sizing and enters the risk envelope, so an uncertain mark cannot silently support leverage.
16.4 Commitments and Capital Calls
Private funds do not take capital when you offer it. They call it when they choose, and an uncalled commitment is a liability that must be reserved against.
Object
Detail
Commitment
A promise of capital, mostly unfunded, with an expected call schedule and a hard obligation
Call forecast
Probability-weighted schedule of when capital will be demanded, from fund stage and historical pacing
Liquidity reserve
Capital held against expected calls, unavailable for deployment. A first-class draw on the capital engine
Default consequence
Failing a capital call typically forfeits the position. The reserve is not optional
J-curve
Early negative returns from fees before value accrues, modelled explicitly so early marks are not misread
Distribution forecast
When capital and gains are expected back, feeding the liquidity ladder
This is the deepest structural difference from everything else in the platform. Every other position can be exited at some price. A private commitment cannot be exited at all for years, and it can demand more capital on someone else&apos;s schedule. The capital engine treats an unfunded commitment as a hard reservation, never as available capital.
17. Asset Class Coverage
What the platform can reach, what each class requires, and what remains genuinely out of scope.
17.1 Continuously Priced
Class
Requires
Status
Crypto spot and perpetual
The executable graph and eight paths
Reachable now
Dated futures
Term structure for fair value and roll
Reachable with the valuation plane
FX spot and forward
Term structure for points
Reachable with the valuation plane
Listed equities
Corporate actions, settlement bridge
Reachable with the valuation plane
Listed options
Volatility surface
Reachable with the valuation plane
Government and corporate bonds
Term structure, credit, matrix pricing
Reachable with the valuation plane
Credit default swaps
Credit engine, ISDA documentation workflow
Reachable, with legal workflow outside the platform
ETFs and index products
Basket decomposition, creation and redemption mechanics
Reachable with the valuation plane
17.2 Event and Information Driven
Class
Requires
Status
Prediction markets
World model, event resolution engine, base rates, resolution source monitoring
Reachable with cognition. The best fit for small capital
Sports and event betting
Same machinery, different domain knowledge
Reachable with cognition
Insurance-linked and catastrophe
Physical index modelling, event resolution
Reachable with ingestion and cognition
Weather and freight derivatives
Physical index ingestion
Reachable with ingestion
Carbon and renewable certificates
Registry integration, vintage and additionality modelling
Reachable with ingestion
17.3 Illiquid and Private
Class
Requires
Status
Private equity and venture
Illiquid valuation, commitments, capital calls, secondary access, diligence workflow
Reachable, and a distinct operating mode — see 17.5
Private credit
Credit engine, covenant tracking, borrower monitoring, cashflow forecasting
Reachable with the valuation plane
Real estate
Appraisal modelling, illiquidity budget, financing structure
Reachable with the valuation plane
Royalties and IP
Cashflow forecasting for irregular contingent streams
Reachable with the valuation plane
Litigation finance
Outcome probability, duration modelling, cashflow forecasting
Reachable with cognition and valuation
Art and collectibles
Provenance, condition, authentication, auction cycle, comparables
Reachable, with authentication genuinely outside the platform
17.4 Physical
Class
Requires
Status
Physical commodities
Storage cost, delivery location, grade differential, seasonality, logistics
Reachable with ingestion and a logistics engine
Product and retail arbitrage
Shipping cost and time, customs, storage, spoilage, returns, marketplace fee structures
Reachable with a logistics engine. A genuinely different cost model
Auction-traded assets
Bidding strategy, winner&apos;s curse adjustment, reserve estimation
Reachable with an auction engine
17.5 Private Markets Are a Second Operating Mode
Everything else in the platform assumes a continuously priced, instantly reconcilable position that can be exited. Private assets have no price, no reconciliation against a venue, a capital call schedule set by someone else, and a horizon measured in years. That is not a new venue adapter.
Assumption elsewhere
Private markets reality
Consequence
Position has a price
It has a mark, with a method and a confidence
Portfolio value becomes a distribution, not a number
Reconciles against a venue
Reconciles against a statement, quarterly
Reconciliation cadence differs by orders of magnitude
Can be exited
Locked for years; secondaries at a discount if at all
Liquidity ladder must model it explicitly
Capital is deployed when chosen
Called on the fund&apos;s schedule
Unfunded commitments reserve capital indefinitely
Risk is market risk
Also concentration, manager, vintage and duration risk
The risk envelope needs new dimensions
Horizon is intraday to weeks
Five to twelve years
Multi-horizon allocation is mandatory, not optional
The honest framing is that private markets share the ledger, the wallet, the identity layer, the capital engine and the frontend, and share almost nothing else. It is a second operating mode inside one platform, and building it as though it were another asset class would corrupt the assumptions the fast path depends on.
17.6 Out of Scope, and Why
Class
Why not
Physical delivery commodities requiring storage
Requires physical infrastructure, not software. Financially settled equivalents are in scope
Anything requiring a banking or broker-dealer licence the operator lacks
A regulatory question, not an architectural one, and it must be answered before it is built
Market manipulation adjacent strategies
Excluded by policy, not capability
Assets with no reliable valuation method at all
If no method produces a defensible mark, the position cannot be risk-managed and should not be held
17.7 The Asset Class Registry
Hundreds of asset classes are architecturally reachable because the graph node is class-agnostic and the six valuation engines cover every pricing paradigm. What turns capable into supported is a registry: one record per class that says how it is priced, how it settles, how it trades, and which strategies may touch it. Without it, hundreds of classes is a capability claim without a list.
Field
What it fixes
Example
Valuation engine
Which of the six paradigms prices it
A tokenized bond points at term structure; a venture position at illiquid
Settlement convention
Stage and cycle
Crypto spot instant; equity T+1; private, quarterly statement
Tick and lot rules
What sizes are expressible
Feeds the feasibility gate directly
Trading hours and calendar
When it is live
Continuous, or an exchange session with holidays
Margin regime
How it collateralises
Feeds the cross-margin model in 25.6
Corporate action applicability
Whether the actions engine applies
Equities yes; crypto spot no; tokenized RWA depends on the wrapper
Tax treatment class
Which jurisdiction rules apply
Feeds the tax engine
Eligible strategy families
Which of the ten families may trade it
Microstructure suits liquid spot; carry suits funding-bearing instruments
Hedge instruments
What can offset it, from the hedge map
Feeds Path 4 and divestment
A class with no valuation engine, no settlement convention, or no eligible family cannot be registered, and an unregistered class cannot be traded. The registry is the gate between architecturally reachable and actually supported, and it is what makes hundreds of classes an enumerable set rather than an aspiration.
17.8 Settlement Calendars
The settlement timeline handles T+0, T+1 and T+2, but a T+1 that lands on an exchange holiday is really T+2, and a market closed for a national holiday is not a venue outage. At hundreds of classes across many jurisdictions this becomes real, governed data rather than an assumption.
Holds
Used by
Per-market trading sessions and holidays
Strategy activation, feasibility, staleness
Per-class settlement cycles adjusted for holidays
The settlement timeline and reservation
Half-days and early closes
Quote rate budgets and liquidity expectations
Cross-jurisdiction differences
Mirror timing and cross-region execution
Roll and expiry dates
Term structure and dated futures
A market being closed is a known state, not a failure. Distinguishing a holiday from an outage is what keeps the platform from quarantining a venue that is simply shut.
18. Feasibility and Small Capital
At small capital the binding constraint is not latency and not signal quality. It is whether a trade can execute at all, and whether its edge survives a fee floor. Version 8.0 had no layer that asked this.
18.1 The Feasibility Gate
Sits ahead of the profitability filter, in the hot path, and answers a cheaper question first: can this execute at my size?
Check
Against
Failure
Minimum order size
Venue minimum notional and minimum quantity
Reject before any further computation
Tick and lot granularity
Whether the intended size is expressible at all
Reject or round, per policy
Fee floor
Fixed fee component against expected gross edge
Reject if the fee floor consumes the edge
Gas or network cost
For on-chain execution, current cost against edge
Reject, and record the threshold
Withdrawal economics
Whether capital can leave the venue for less than the profit
Reject the venue for this size entirely
Depth at size
Whether the book can absorb the intended size at the assumed price
Reduce size or reject
Settlement viability
Whether capital returns in time to be useful
Reject or reprice for the delay
At two hundred dollars this gate rejects the overwhelming majority of detected opportunity, and knowing precisely which fraction survives is the entire game. It is also the highest-value observability signal at small capital — the distribution of feasibility rejections tells you exactly which venues and which strategies are worth having at all.
18.2 The Arithmetic, Stated Plainly
Quantity
At $200
Implication
Infrastructure at Phase 0–4
~$1,015 per month
Five times capital, monthly
Round trip at 10 bp taker
$0.40
0.2 percent of capital per round trip
Typical crypto withdrawal fee
$1–25
Can exceed the position
Minimum order at many venues
$1–10 notional
Binds on most cycle legs
Break-even on hosting alone
~500 percent monthly
Not achievable by any legitimate strategy
Two hundred dollars is a correctness harness. It proves the plumbing with real money at risk, which is exactly the purpose of the Phase 3 gate — the first time real money is at risk — and it is not an engine. Treating it as an engine would mean either accepting losses as the cost of learning — which is legitimate if named — or reaching for strategies whose apparent edge is a measurement error.
18.3 Where Small Capital Genuinely Wins
Arena
Advantage
What it needs from v9
Prediction markets
Thin books, research-driven edges, institutions largely absent because they cannot size in
World model, event resolution, base rates, resolution source monitoring
Maker rebates on quiet pairs
Providing liquidity earns rather than pays. Size is not the constraint
Market making already built; feasibility gate to find the pairs
Funding capture
Spot long against perpetual short collects funding with no repeated crossing cost
Already reachable. Needs the feasibility gate to confirm the fee floor clears
Small-notional inefficiencies
Precisely what institutions ignore because they cannot move size through them
Feasibility gate inverted — find what is only viable at small size
Long-horizon private positions
No latency requirement, and access is the constraint rather than speed
Valuation plane, commitments, liquidity ladder
The common thread is that the edge is informational or structural rather than latency-based. The version 8 architecture was optimised for a constraint that does not bind at this capital level, and the cognitive and valuation planes are what address the constraint that does.
18.4 Compounding Policy
At small capital the reinvestment schedule is not an accounting detail; it is a strategy in its own right, and there was no layer that reasoned about it.
Decision
Detail
Reinvestment cadence
How often realised profit is redeployed against the transaction cost of redeploying it
Fee tier accumulation
Volume thresholds that reduce fees. Reaching one can be worth more than the trades that reach it
Threshold crossing
Capital levels at which new venues, strategies or sizes become feasible. Approaching one is worth planning for
Withdrawal drag
Every withdrawal resets compounding. The frontend should show that cost, not hide it
Minimum viable scale
The capital level at which the platform becomes economically sensible, computed and shown honestly
19. The Strategy Universe
Ten families. Each has a distinct alpha source, holding period, capacity profile and failure mode. A strategy is a parameterised instance of a family applied to a universe.
Family
Alpha source
Horizon
Tier
Capacity
Market making
Bid-ask spread, minus adverse selection
ms to minutes
0
Bounded by inventory risk and quote rate
Arbitrage
Price inconsistency across venues, assets, representations
ms to days
0
Bounded by opportunity frequency and capital placement
Microstructure
Order book imbalance, queue dynamics, trade flow
ms to seconds
0
Small. Decays fastest under crowding
Short-horizon reversion
Overreaction to order flow
seconds to minutes
1
Moderate
Momentum and trend
Continuation across horizons
minutes to weeks
2–3
Large. The most capacity-tolerant
Statistical arbitrage
Mean reversion in spreads, pairs, baskets
hours to days
2–3
Moderate. Crowding-sensitive
Carry
Funding, basis, roll, term structure
days to weeks
3
Large but rate-dependent
Event-driven
Scheduled and unscheduled information events. Now including prediction markets
minutes to days
1–2
Episodic. Informational edge — the small-capital family
Volatility
Implied versus realised, surface shape, dispersion
days to weeks
2–3
Moderate
Execution alpha
Improving the fills of every other family
within an order
0
Scales with total flow
19.1 How Ten Thousand Is Reached
   10 families x ~15 variants x ~12 universes x ~6 parameterisations = ~10,800
This decomposition exposes the central statistical problem. Ten thousand strategies are not ten thousand independent bets. Variants within a family are correlated, parameterisations of one variant are highly correlated, and families correlate under stress. Effective breadth — the number of genuinely independent bets — is likely in the low hundreds. The platform accounts in effective breadth, not strategy count, and the hypothesis generator in Section 14 exists specifically to raise it.
19.2 Evaluation Tiers
Tier
Cadence
Runs on
Share
Families
0 — Hot
Every relevant event, microseconds
Node, pinned cores
~10%
Market making, arbitrage, microstructure, execution alpha
1 — Fast
Sub-second, event-driven
Node, lower-priority core
~20%
Short reversion, fast event-driven
2 — Warm
Seconds to minutes
Node background thread
~30%
Statistical arbitrage, intraday momentum
3 — Slow
Minutes to hours
Cloud Run
~35%
Trend, carry, volatility, factor
4 — Batch
Daily
Cloud Run Jobs
~5%
Rebalancing, long-horizon allocation
A node carries roughly 3,600 strategies of which about 1,000 sit in the hot tier against a cap of 1,200. That cap is the count at which measured p99 evaluation reaches seventy percent of the 90 µs budget.
20. Strategy Lifecycle at Scale
Ten thousand strategies cannot be approved by hand. Promotion is automated, which makes the statistics the hardest part of the platform. Test ten thousand candidates and roughly five hundred will show two-sigma significance by chance alone. Ranking by backtest Sharpe reliably selects the luckiest noise.
20.1 The Statistical Gate
Control
Mechanism
What it prevents
Deflated Sharpe ratio
Adjusts observed Sharpe for trial count, backtest length, skew and kurtosis
An in-sample number mistaken for evidence
Purged, embargoed cross-validation
Removes training samples overlapping test labels; embargoes a gap after each fold
Lookahead leakage through overlapping horizons — the commonest silent error
Cumulative trial accounting
500 trials per family per quarter, corrected against lifetime count, never per batch
A parameter sweep laundering itself by being split up
Untouched holdout
A period never seen during research, unlocked once per family
Overfitting to the validation set
Live canary
Minimum live period at minimal capital, consistent with holdout
Everything the previous four missed. Reality cannot be overfitted
Capacity estimate
Capital level at which the edge decays, from depth and impact modelling
A real edge funded past the point it becomes a loss
Hypothesis linkage
A strategy must express a supported hypothesis with a stated mechanism
Variation without novelty; correlated near-duplicates
20.2 The Promotion Pipeline
Stage
Gate
Automated
Generated
From a supported hypothesis, a family template, or a parameter sweep
Yes
Backtested
In-sample above a floor. Cheap filter, no significance claimed
Yes
Validated
Purged CV, deflated Sharpe above threshold given cumulative trials
Yes
Holdout tested
Untouched period within tolerance of validation
Yes
Capacity assessed
Estimate produced, maximum allocation derived
Yes
Family approved
A human approved the family, its template and thresholds — once, not per strategy
No — human, hardware key
Canary
Live at minimal capital, consistent with holdout
Yes
Funded
Allocator assigns up to capacity, weighted by belief confidence
Yes
Monitored
Continuous decay detection against the live band. Counterfactual scoring of its declined intents
Yes
Demoted or retired
Outside band, or capacity or crowding deteriorates
Yes
Humans approve families and thresholds, not strategies. A person who approves ten thousand individual strategies has approved none of them meaningfully.
20.3 Decay and Retirement
Signal
Response
Live Sharpe below the lower band of its holdout interval
Reduce allocation. Flag for review
Realised slippage exceeds model persistently
Reduce capacity estimate; reallocate
Family correlation rises above its band
Tighten family caps. Crowding is the likely cause
Strategy has not fired within its expected frequency band
Investigate. Usually a data or universe change
The causal edge it expresses fails its conditions
Reduce before performance degrades, not after
Sustained underperformance beyond threshold
Retire. State archived; slot freed
Retirement is as automated as promotion and matters as much. A platform that only adds strategies accumulates dead weight consuming evaluation budget, message rate and capital while contributing nothing.
21. AI Training
Ten model classes, trained centrally from streaming statistics rather than from a tick archive, exported as signed ONNX artifacts, and shipped to every region. Every model estimates a cost, a probability or a regime. None predicts price direction as a decision.
Model
Predicts
Class
Update
Feeds
Slippage estimator
Expected slippage at size, per venue and instrument
boosting / MLP
online, every fill
Profitability filter, sizing
Fill-time dispersion
Expected arrival spread per venue combination
quantile sketch
streaming
Latency equalisation, sizing
Adverse-selection premium
Cost of a resting order, per venue
regression
exp. weighted
Quote spread, passive thresholds
Quote honour rate
Probability a firm quote is honoured, per counterparty
beta-binomial
conjugate
Firm-quote path gating
Basis convergence
Expected time and path to convergence
sequence model
periodic refit
Carry, holding-period risk
Regime classifier
Trending, mean-reverting, volatile, calm — now feeding the belief state
classifier
refit from reservoir
Strategy activation, allocation
Book pressure
Short-horizon direction from book shape
small MLP / CNN
refit from snapshots
Microstructure, quote skew
Volatility and correlation
Realised volatility and factor structure
EW covariance
streaming
Sizing, exposure limits
Leg-ordering policy
Aggressive or passive per leg, inside bounds
small RL policy
offline PPO, in the simulator
Leg coordinator
Anomaly detector
Unusual market or transfer behaviour
autoencoder
periodic refit
Alerting, transfer gate
21.1 Training Without an Archive
Source
What it is
Size
Streaming estimators
Welford moments, exponentially weighted covariance, t-digest quantiles, count-min, HyperLogLog. Maintained in the node in constant memory
~320 KB for 200 features
Reservoir samples
A fixed-size, recency- and regime-weighted sample standing in for the population
10,000 rows, not 10 billion
Event-anchored snapshots
Book state at every own order, fill, quote, veto and unwind. Labelled examples anchored to outcomes
~100 MB per day, 90-day roll
Episodes
Compressed state with outcome and surprise. New in v9, and the richest training signal for regime and belief models
single-digit GB per year
Counterfactuals
Scored declined paths. Training signal for sizing and gating models
batch
Total sufficient statistics behind all ten models is single-digit megabytes, against terabytes a year of raw capture. The fidelity loss is bounded and monitored: every estimator declares an error bound, and one that drifts past it marks every model depending on it as degraded. A model class proven sensitive to full fidelity is fitted from a fetched historical extract instead — resolved through a data reference, cached for the campaign, then deleted.
21.2 The Pipeline
Stage
Service
Work
Runtime
Features
feature-pipeline
Training frames from statistics checkpoints, reservoirs, snapshots and episodes, using the same feature definitions the node uses
polars, arrow — daily
Train
trainer
burn with autodiff on GPU for neural models; linfa for regression, trees and clustering. Restartable, so spot is safe
Cloud Run GPU or GCE spot — daily to weekly
Evaluate
eval-harness
Candidate against incumbent on held-out folds. Drift, calibration, error against the declared bound
on candidate
Promote
model-promoter
Emits the ONNX graph as protobuf, signs it, publishes to Artifact Registry, records the version in Spanner
prost, ring
Deploy
policy-distributor
Node verifies the signature and swaps the model set by atomic pointer. Trading never pauses
Pub/Sub
The registry is Spanner rows plus Cloud Storage plus Artifact Registry. Vertex AI was removed when the platform went Rust-only; with a Rust training binary it is a container scheduler with extra concepts attached.
21.3 Rust for Machine Learning — Honest Assessment
Rust is not at parity with Python for machine learning in general. It is entirely adequate for what Algorik trains: every model is a small tabular or short-sequence model measured in kilobytes to low megabytes. None is a large transformer.
Where Rust is not adequate
Assessment
Training a large language model
Not competitive. Not needed — the platform uses a hosted model through an API for hypothesis proposal and extraction
Fine-tuning a foundation model
Possible via burn with LibTorch, but painful. Not needed
Very large distributed training
Immature. Not needed at this model size
Exotic architectures from recent papers
Reference implementations are Python. Reimplementation cost is a legitimate reason to prefer a simpler architecture, not to break the rule
Training and inference share the tract crate and the same feature computation code. Training-serving skew — among the most common and expensive failures in applied ML — is removed structurally rather than by discipline.
22. The Data Policy — Pass-Through, Not Accumulation
Algorik does not own a copy of the world&apos;s data. The stream passes through, updates what the platform needs to decide and to learn, and is discarded. What remains is sufficient statistics, episodes, the platform&apos;s own irreplaceable records, and pointers to history that lives elsewhere.
This is a deliberate rejection of data gravity. A platform holding years of raw ticks and text is anchored to wherever that data sits. It cannot change cloud, schema or vendor without a migration measured in months, and it pays storage and query costs on a corpus that is overwhelmingly stagnant.
22.1 Retention Classes
Class
What
Retained?
Size
Transient
Raw ticks, book deltas, quote updates, source text
No. Bounded ring measured in seconds, then gone
Constant
Derived state
Features, moments, covariance, sketches, reservoirs
Yes, in memory. Fixed size regardless of throughput
Single-digit MB per model class
Irreplaceable
Own orders, fills, intents, verdicts, quotes, receipt timestamps, transfers
Yes, permanently. Only Algorik has these
Grows with activity, not volume
Compact derived
Per-strategy returns, family correlations, dispersion by venue pair, solver deltas, counterfactual scores
Yes. Series, not observations
MB to low GB
Episodic
Compressed state with outcome, indexed for retrieval
Yes, indefinitely. Compressed meaning
Single-digit GB per year
Semantic
Entities, relations, causal edges, beliefs, extracted facts
Yes. The world model
Low GB
Event-anchored
Book state at each own order, fill, quote, veto, unwind
Yes, 90 days rolling
~100 MB per day
Fallback series
One-minute OHLCV for instruments in an active class or universe
Yes, three years. Insurance against a source withdrawing its archive
Few hundred MB
Referenced
External market history, filings, registries
No. A manifest with source, range and content hash. Fetched on demand
Bytes per reference
The distinction doing the work is between what only Algorik knows and what the world already knows. The first is kept because it cannot be recovered. The second is not kept because it can. Version 9 adds two classes — episodic and semantic — and both are compressed meaning rather than retained observation.
22.2 Sufficient Statistics
Needed
Streaming method
Memory
Mean, variance, higher moments
Welford, extended for skew and kurtosis
O(1) per series
Covariance and correlation
Exponentially weighted online update
O(k²) — 200 features is ~320 KB
Quantiles
t-digest or KLL sketch
Few KB per distribution, bounded error
Frequency and cardinality
Count-min sketch, HyperLogLog
Kilobytes, bounded error
Representative samples
Weighted reservoir sampling
Fixed row count
Linear and logistic models
Recursive least squares, online gradient
O(k) parameters
Factor structure
Streaming PCA — Oja&apos;s rule or incremental SVD
O(k × components)
22.3 Data References
   DataReference { source, endpoint, symbols, range, schema_version,
                   content_hash, retrieved_at, cost_estimate, availability }
Field
Effect
Content hash
A re-fetch producing a different hash means the source revised its data. The backtest that used the original is flagged, not silently invalidated. The single most important field
Cost estimate
The research scheduler knows what a run will cost before it runs, and batches or defers accordingly
Availability
Unreliable sources are deprioritised. A data class with only one viable source is a concentration risk, and a universe with fewer than two cannot be promoted past validation
Schema version
Vendors change formats. The reference records what shape the data was in when used
22.4 Fetch-on-Demand for Research
   campaign starts -> resolve references -> fetch into TTL cache -> verify hashes
   -> backtest and purged CV over the cache -> emit results, statistics, manifest
   -> cache expires and is deleted.  What persists: the manifest and the results.
Risk
Mitigation
Vendor withdraws historical access
Two registered sources required per data class. Bar-level fallback retained for three years on traded instruments
Source revises history after use
Content hashes detect it. Affected validations flagged for re-run
Sketch or reservoir error affects a model
Bounds are declared and monitored. A sensitive model class is fitted from a fetched extract instead
Research is slower than a local copy
Campaign-scoped caching and batching. A permanent cost, accepted deliberately
Regulatory demand for data not retained
Named extracts fetched and retained deliberately. The manifest proves what was used at the time
23. The Optimisation Plane
Six combinatorial workloads, each sized to fit current quantum hardware, each with a classical solver scored against it on every instance. Finding an opportunity is cheap; deciding which subset to fund with shared capital, and where that capital should physically sit, is NP-hard.
Workload
Decides
Variables
Cadence
Family allocation
Capital across 128 correlation-clustered families under cardinality and an effective-breadth objective
128 + encoding
hourly, adaptive
Regime-conditional weighting
How that allocation shifts with the regime the belief state reports
regime × family
on regime change
Cycle selection
Which overlapping arbitrage cycles to fund when they compete for inventory
80–150
1–5 min per region
Path assignment
Which of eight mechanisms each funded cycle uses — solved jointly with selection
per cycle-path pair
same run
Capital and mirror placement
Where capital and inventory sit ahead of opportunity
60–110 per region
15–60 min
Multi-horizon reconciliation
How microsecond arbitrage and multi-year private positions coexist
horizon × family
daily
23.1 Allocation Across Ten Thousand Strategies
   LEVEL 1  cluster ~10,000 strategies into 128 families by return correlation    classical, daily
   LEVEL 2  allocate across 128 families, cardinality-constrained, breadth objective  quantum + classical, hourly
   LEVEL 3  distribute each family&apos;s budget across its strategies by capacity       classical, hourly
The decomposition mirrors the real structure. Strategies within a family are near-substitutes, so allocating precisely among them adds little. Allocating across families is where diversification is won or lost, and it is exactly the size current hardware handles.
23.2 Hardware
Capability
Current
Trajectory
Processors
Heron r1 133 qubits, Heron r2 and r3 156, Nighthawk 120 with 218 couplers on a square lattice. ~2,477 qubits across 17 QPUs
Nighthawk scaling to three linked modules
Circuit depth
Up to 5,000 two-qubit gates on Nighthawk
7,500 by end of 2026, 10,000 by 2027, 15,000 by 2028
Connectivity
Square lattice, four-degree. Over 20 percent more couplers than Heron, ~30 percent more circuit complexity with fewer SWAPs
Long-range couplers to 1,000+ connected qubits
Fidelity
Two-qubit gate fidelity above 99.9 percent on more than half of tested pairs. ~330,000 CLOPS fleet-wide
Incremental gains
Fault tolerance
Loon proves error-correction components
Starling in 2029: 200 logical qubits, 100 million gates
Every workload is sized to roughly 200 binary variables, which is what current hardware handles directly. That sizing is a design constraint, not a coincidence.
23.3 Regime-Conditional Allocation
Regime signal
Favours
Breadth expanding, volatility contained
Momentum and trend
Volatility elevated, breadth narrow
Short reversion, market making at wider spreads
Funding stable, curves steady
Carry, basis, cash and carry
Dispersion high across correlated names
Statistical arbitrage, relative value
Event density high
Event-driven, prediction markets
Regime uncertain — belief distribution wide
Reduce everything; favour arbitrage whose edge does not depend on regime
When the platform does not know what regime it is in, the correct response is not to guess but to concentrate in strategies whose edge is structural. Arbitrage is regime-agnostic; momentum is not.
23.4 Multi-Horizon Reconciliation
Horizon
Character
Liquidity
Allocation treatment
Microseconds to minutes
Arbitrage, market making, microstructure
Immediate
Against available inventory, recycled continuously
Hours to days
Statistical arbitrage, event-driven, reversion
Same or next day
Against deployable capital
Weeks to months
Trend, carry, volatility
Days to unwind
Against capital not reserved for calls
Years
Private equity, credit, real assets, royalties
Effectively none
Reserved capital plus unfunded commitment liability
23.5 The Hybrid Pattern
   instance -> CLASSICAL (Rust: annealing, tempering, tabu) -> solution + optimality gap
            -> ROUTE? only if the gap is materially non-zero
                  -> QUANTUM (Rust -> QASM3 -> IBM Runtime REST)
            both scored on the same objective; best feasible becomes policy
            delta recorded per problem class and instance size
The classical solver is standard engineering, not a lack of confidence: every production optimiser needs a fallback under a deadline. The routing gate is both economic and scientific — instances with a provably small classical gap never reach a QPU, which concentrates quantum usage where advantage is plausible and the measurement therefore means something.
23.6 Adaptive Cadence and Sequencing
Signal
Action
Whitelist hit rate falling
Trigger cycle selection
Family correlations shifting
Trigger clustering and family allocation
Regime belief changed
Trigger regime-conditional weighting
Inventory deviation rising
Trigger placement
Nothing changed
Do not run. This is the saving
Build order: classical solvers and the comparison harness in weeks one and two, so the plane is fully functional on classical from the start. The quantum path plugs into a working scorer as a second solver. Quantum can never block the plane.
23.7 Scenario and Stress
Method
Reaches
Historical replay
Shocks that occurred, at the correlations that held then
Correlation stress
Shocks scaled beyond history, assuming the same structure
Causal propagation
A shock at a driver, propagated through mechanisms, reaching exposures no correlation would connect
Adversarial scenario
The simulator constructs the worst plausible sequence given current positions
24. IBM Quantum From Rust
Qiskit is a Python SDK. It is not the only access path. Qiskit Runtime exposes a REST API accepting OpenQASM 3 strings, and IBM documents that the workflow runs from any language capable of REST calls. The Python SDK is a convenience layer over an HTTP interface, and the interface is what Algorik uses.
  POST https://quantum.cloud.ibm.com/api/v1/jobs
    Authorization: Bearer <token>   Service-CRN: <instance>   IBM-API-Version: <date>
    { "program_id": "sampler", "backend": "ibm_...",
      "params": { "pubs": [[ "OPENQASM 3.0; ..." ]], "resilience_level": 1 } }
Step
Work
Effort
QUBO to cost Hamiltonian
Off-diagonal terms become ZZ couplings, diagonal terms become Z. Direct matrix read
Trivial
QAOA ansatz
Per layer: RZZ weighted by couplings, RZ by diagonals, RX mixer. Hadamard initial state. Fixed structure
Low
Routing to topology
Linear SWAP network — odd-even transposition — realises all-to-all deterministically
Medium
ISA compilation
Path A: IBM Transpiler Service over REST. Path B: Rust ISA emission with a cached SWAP network
Medium
OpenQASM 3 emission
Text serialisation. Only a small subset of the specification is needed
Low
Submission, sessions, polling
reqwest. Sessions avoid re-queueing on every parameter update
Trivial
Parameter outer loop
SPSA over gamma and beta. Noise-tolerant, ~30 lines with argmin
Low
Start with Path A. It removes the hardest work from the critical path and keeps the dependency inside IBM. For a fixed problem size on a fixed backend the routing pattern is identical every run and can be cached. Estimate three to five weeks. The documented fallback is one isolated Python job behind a strict JSON boundary, and per Section 23.6 it cannot block the plane in any case.
25. Capital, Risk and Tax
25.1 The Decision Chain
Layer
Decides
Cadence
Can say yes
User mandate
Capital, risk tolerance, permitted families, drawdown ceiling, liquidity floor, exploration share, jurisdiction
manual
yes — ultimate
Capital engine
Deployed share against reserve, exploration budget, split across families and regions, transfer sizing and routing
1–5 min
yes
Risk engine
The exposure envelope at ten levels including causal-driver concentration
30 s – 5 min
no — constrains only
Optimiser
Allocation inside the envelope, conditioned on regime and horizon
adaptive
yes
Capital grants
Bounded, expiring permission per family per region, shipped outward
per run
carries the yes
Regional gate
Whether a specific intent may proceed
microseconds
no — veto only
Transfer gate
Whether a specific movement may proceed
per transfer
no — veto only
Only three layers can say yes: the user, the capital engine, and the optimiser inside the envelope. Everything closer to the money can only say no. A grant expires rather than being revoked — if a region loses contact, its grants age out and its capacity to deploy shrinks on its own.
25.2 The Capital Engine
Question
Answer
How much to invest
Total splits into deployed, reserve and exploration. The reserve covers withdrawal liquidity, settlement float, margin headroom, unfunded commitments and opportunity capacity
How much to move
Enough to restore target, never more. Over-moving pays fees and settlement time on capital that was coming back
Where to move it
Cheapest signed corridor that arrives in time, tracked as in-transit inventory
When not to move
When direction gating will rebalance a mirror through ordinary trading, or when transfer cost exceeds the expected gap
How much to explore
A stated share of deployed capital, allocated by uncertainty rather than expected return, accounted separately
What to reserve
Expected capital calls, probability-weighted, held against unfunded commitments and unavailable for deployment
25.3 The Risk Envelope
Level
Limit
Set by
Enforced
Per user
Total capital, drawdown ceiling, permitted families, liquidity floor, unfunded commitments
Mandate
capital engine
Per strategy
Position, notional and loss cap, bounded by capacity estimate
Allocator
before netting
Per family
Aggregate exposure and drawdown across the cluster
Quantum allocator
on net intent
Per instrument
Net position across all strategies and users
Risk policy
on net intent
Per asset class
Aggregate signed and gross notional
Risk policy
on net intent
Per venue
Exposure and concentration at one counterparty
Risk policy
on net intent
Per factor
Systematic exposure, decomposed at fill time
Portfolio construction
on net intent
Per causal driver
Exposure to a shared cause, surfaced from the causal graph
Cognition
on net intent
Per region
Aggregate exposure and capital deployed
Risk policy
on net intent
Global
Gross, net, leverage, drawdown breaker
Human ceiling
continuously
Per causal driver is new. Positions that look diversified by instrument and by factor can still share a mechanism, and only the causal graph can see it. This is the concentration that ends firms.
25.4 The Liquidity Ladder
Rung
Horizon
Cost to liquidate
Cash and stablecoin at venue
Immediate
Zero
Liquid spot and perpetual
Seconds
Spread and fee
Listed equities and futures
Same day
Spread, fee, impact
Bonds and less liquid listed
Days
Wider spread, dealer inventory
Resting and anchored positions
Days, by abandoning the cycle
Opportunity plus unwind
Private credit and real assets
Months, if at all
Substantial discount
Private equity commitments
Years, or a secondary at a discount
Large discount, possibly no bid
A withdrawal is served from the top downward. It never forces a position closed at a bad price to meet a routine request — the reserve exists so it does not have to.
25.5 Tax
Capability
Effect on decisions
Lot tracking
Every position carries lots with acquisition date, basis and jurisdiction
Lot selection
Which lot to close changes the tax outcome materially
Holding period
A position days from long-term treatment may be worth holding through a marginal exit signal
Wash sale awareness
A loss harvested and immediately re-established may be disallowed. The gate knows
Jurisdiction
Treatment differs by domicile, class and venue. The mandate carries it
Harvesting
Deliberate loss realisation where it is worth more than the position
After-tax sizing
Two trades with identical pre-tax edge can differ materially after tax
Rules are jurisdiction-specific and change. The architecture provides lot tracking, holding-period awareness and a pluggable treatment model; the specific rules are configured, not hard-coded.
25.6 The Cross-Margin Model
Inventory has an encumbered stage and reserves carry margin headroom, but nothing models what collateralises what across margin regimes. At hundreds of classes a position in one place can support or endanger a position in another, and the platform must know which.
Concern
Model
What collateralises what
A collateral graph: which assets back which positions, with haircuts, per margin regime
Portfolio versus isolated margin
Some venues net exposure across positions; some ring-fence each. The model records which, per venue
Rehypothecation
Whether posted collateral can itself be reused, and the risk that creates
Correlated collapse
Collateral and the position it backs falling together — the classic margin spiral, surfaced through the causal graph
Cross-venue collateral
Whether a position at one venue can margin a position at another, which is rare and dangerous
Liquidation cascade
What a forced liquidation at one venue does to margin elsewhere, modelled before it happens
The cross-margin model feeds the risk envelope and the liquidity ladder. A position is only as safe as the collateral behind it, and collateral that can fall with the position it backs is not really collateral. This is where the per-causal-driver limit earns its place — shared-mechanism collateral is the concentration that a notional view cannot see.
26. Strategy Execution at Scale
Evaluating ten thousand strategies by iteration is impossible in a microsecond budget. The answer is to stop iterating.
26.1 A Strategy Is a Specification
   Strategy { family, universe, features (declared dependencies), predicate,
              sizing, tier, capacity, beliefs (consumed), hypothesis (expressed) }
Because a strategy declares its feature and belief dependencies rather than reading whatever it likes, the platform knows exactly which strategies an event can affect. The predicate language is deliberately total: comparisons, arithmetic, boolean operators and bounded windowed aggregates. No loops, recursion or calls into arbitrary code. A Turing-complete language makes evaluation cost impossible to bound statically, which destroys the budget guarantee everything rests on. Expressiveness is added through features, computed once and shared.
26.2 Compilation
Step
Effect
Feature deduplication
Ten thousand strategies reference ~500 distinct features. Each computed once per event
Common subexpression elimination
Predicates share subterms. A comparison used by 400 strategies evaluates once. Five to tenfold reduction
Predicate compilation
Branch-free evaluation over packed vectors, 20–50 ns each
Subscription index
Inverted index from feature to dependent strategies. Wake only subscribers
Tier partitioning
Only tiers 0 and 1 in the hot loop. Hot tier capped at 1,200; a plan exceeding budget fails to compile
Universe bitmaps
Instrument relevance is a bit test
Layout packing
Hot-tier state cache-line aligned, ordered by evaluation sequence
Belief binding
Strategies declare which beliefs they consume; stale beliefs reduce their size automatically
Feasibility pre-binding
Per-venue minimums and fee floors compiled in, so infeasible combinations never evaluate
26.3 The Loop and Budget
   event -> book update (5-20 features dirty) -> recompute dirty features
   -> subscription lookup -> universe bit test -> shared subexpressions
   -> predicate evaluation -> belief and feasibility -> sizing -> INTENTS -> netting
Stage
Budget
Feature recompute, 5–20 dirty
< 12 µs
Subscription lookup and universe filter
< 4 µs
Shared subexpressions, ~200 unique
< 10 µs
Predicate evaluation, ~800 woken
< 32 µs
Belief and episodic digest lookup
< 3 µs
Feasibility gate
< 3 µs
Sizing and intent generation
< 6 µs
STRATEGY ENGINE TOTAL
< 70 µs
Seventy microseconds on a hot path already spending roughly six hundred. The venue round trip remains dominant by an order of magnitude.
26.4 Why the Budget Holds
Concern
Answer
All ten thousand subscribe to one feature
They cannot. Universe bitmaps mean a strategy wakes only for instruments it trades, and the hot tier is capped. A set that would breach the budget fails to compile
A strategy is expensive
Predicates are total expressions with a bounded complexity budget enforced at compile time. Expensive computation belongs in a feature
The set changes constantly
Recompilation is off the hot path. The new plan swaps in by pointer. Trading never pauses
A volatility burst wakes everything
Firing rate is bounded by netting and the gate, not by evaluation. Evaluation cost is roughly constant
Memory
Ten thousand strategies at a few KB of packed state is tens of MB. Hot-tier state is a fraction, cache-line packed
27. Intent Netting
Without netting the platform sends one order per firing strategy. Two hundred strategies agreeing on a direction would send two hundred orders — breaching rate limits, paying the spread two hundred times, colliding with itself, and telegraphing intent.
   evaluation produces N intents -> group by (instrument, venue) -> net signed size
   -> record contributor vector [(strategy, signed_size), ...]
   -> ONE net intent -> gate -> ONE order -> fill attributed pro-rata
Problem
Without netting
With netting
Spread cost
Paid once per strategy
Paid once for the net position
Self-trade
One strategy&apos;s buy crosses another&apos;s sell. A regulatory problem and a pure loss
Opposing intents cancel internally. Neither reaches the venue
Rate limits
Order rate scales with strategy count
Scales with distinct instruments, which is bounded
Market impact
Two hundred small orders in one direction is a visible signature
One order, sized and sliced deliberately
Gate cost
Two hundred evaluations
One evaluation on the net
27.1 Internal Crossing
Rule
Detail
Crossing price
The prevailing mid at the netting instant, recorded and auditable. Never a price either side chose
Attribution
Both strategies receive their full intended fill at the crossing price. Neither is disadvantaged
Cap
Forty percent of gross intent per instrument per interval. Above that a persistent internal market forms whose marks drift from reality; below roughly twenty percent free netting value is left unclaimed
Audit
Every internal cross is a ledger entry with both contributors and the price. A regulatory expectation, not an optimisation detail
Internal crossing is where a meaningful share of the value of running many strategies is realised. Strategies that disagree cost nothing to run together because their disagreement never reaches a venue. The netting ratio — gross intent over net order volume — is the single best summary of whether the strategy set has genuine diversity.
27.2 Rules Across Venues and Cycles
Case
Handling
Same instrument, same venue
Netted. One order
Same instrument, different venues
Not netted by default — different executions at different prices. The router may consolidate onto the best venue if strategies did not specify one
Same underlying, different representations
Not netted. Spot and perpetual are different instruments with different risk. Exposure aggregated for risk; orders separate
Arbitrage cycle legs
Never netted with directional intents. A cycle leg is part of an atomic set. Netting it would silently break the cycle&apos;s economics. Legs carry a no-net flag
Market making quotes
Quotes read the net directional position and skew accordingly. A directional buy makes the market maker&apos;s bid more aggressive rather than competing with it
28. Hierarchical Risk Aggregation
A risk check must be O(1) in strategy count. Summing ten thousand positions per check would cost more than everything else in the hot path combined.
   every fill updates a fixed set of counters:
     position[instrument]  exposure[class]  exposure[venue]  exposure[region]
     exposure[factor]  exposure[family]  exposure[causal_driver]  gross, net, P&L
   cost: ~9 atomic adds per fill, independent of strategy count
Strategy-level limits are checked before netting, because a strategy that has exhausted its budget must not contribute to a net intent at all. Every other level is checked on the net, once.
28.1 Correlated Exposure
Two hundred momentum strategies each inside its own limit can produce a concentrated aggregate position no individual limit catches.
Control
Mechanism
Factor exposure limits
Positions decomposed onto systematic factors at fill time using a loading matrix shipped as policy
Causal-driver limits
Positions traced to shared mechanisms through the causal graph. Catches concentration factor analysis misses
Family concentration caps
No family may exceed a share of gross exposure regardless of how many strategies are individually within limits
Effective breadth floor
The allocator refuses additional correlated strategies once marginal diversification falls below threshold
Crowding response
When realised correlation between family returns rises above its band, family caps tighten automatically
29. Market Making and Market Creation
29.1 The Quote Loop
   fair value   = reference mid adjusted by microstructure signal and belief
   half spread  = base + volatility term + adverse selection term
   skew         = f(inventory vs target)      shifts both quotes
   size         = f(budget, volatility, queue position value, confidence)
   bid = fair - half_spread - skew      ask = fair + half_spread - skew
   reprice only if the change exceeds the requote threshold
Component
Failure if absent
Inventory skew
Inventory drifts monotonically until the position limit halts quoting
Adverse-selection term
The book fills you exactly when it should not — the classic way market making loses
Volatility term
Quotes are picked off during bursts
Requote threshold
Message rate explodes and the venue throttles or disconnects
Queue position value
Constant repricing destroys queue priority, most of the edge on a lit book
Toxic flow detection
A single informed counterparty extracts the day&apos;s spread capture
Belief weighting
Quoting confidently into a state the platform does not understand
29.2 Quote Rate Management
Quote traffic exceeds order traffic by one to two orders of magnitude, and venues enforce message-rate limits and message-to-trade ratios. Rate management is a hard requirement.
Control
Detail
Per-venue rate budget
A token bucket per session, sized below the venue&apos;s limit with headroom
Priority allocation
Under constraint, instruments are repriced in order of expected value of the update
Threshold adaptation
The requote threshold widens as the budget depletes
Message-to-trade monitoring
Tracked per venue. Quoting narrows if the ratio deteriorates
Mass cancel
A single message withdrawing all quotes on a venue, wired to the kill switch and the drawdown breaker
Quoting and cycle execution share the reservation table. A quote is a conditional commitment of inventory; if it is not reserved, an arbitrage cycle can consume the inventory backing a live quote, and the resulting fill has nothing behind it.
29.3 Market Creation
The highest-margin activity in finance and the most dangerous. Creating a market means being the counterparty when nobody else will be, and doing that without understanding why nobody else will is how a platform becomes exit liquidity rather than the house.
Form
What it is
Prerequisite
Liquidity provision where none exists
Two-sided quoting in an instrument with no continuous market
Valuation plane — you cannot quote what you cannot price
Origination of structured exposure
Constructing a payoff from components and offering it
Volatility surface, term structure, credit
Counterparty of last resort
Taking the other side of a hedge nobody else will price
Causal model of why they will not
Cross-market bridging
Quoting the same exposure in a venue where it is unavailable
World model linking the representations
Prediction market origination
Creating a contract on an event and making both sides
Event resolution engine, base rates, a resolution source
Gate before any market creation
Why
A defensible valuation with method and confidence
Quoting without a price is gambling with extra steps
A causal explanation for the absence of other participants
If the reason is information you lack, you are the counterparty they are avoiding
An adverse-selection model for this instrument class
Origination concentrates adverse selection more than any other activity
Bounded maximum exposure, hard-coded
An originated position may have no exit
Human approval per instrument class
A business decision, not a strategy promotion
30. Arbitrage — The Executable Graph
One family of ten, with the cleanest correctness properties, and shared infrastructure that other families read.
   node   = (asset, venue, representation, settlement_stage)
     representation in { spot, perp, future(expiry), tokenized, etf, synthetic, cash_at(T) }
     settlement     in { instant, T+0, T+1, T+2, in_flight }
   weight = -ln(rate) + ln(1+fee) + slippage(size) + carry + bridge_cost
   a profitable cycle is a NEGATIVE-WEIGHT CYCLE
Negative log rates turn a multiplicative profit condition into an additive one, making shortest-path machinery applicable.
Edge class
Connects
Enables
Conversion
Two assets, same venue
Triangular and quadrangular cycles
Transport
Same asset, two venues, same region
Cross-venue cycles
Mirror
Same asset, two regions
Cross-region without coordination
Basis
Two representations of one underlying
Cash and carry, funding capture, ETF against basket
Equivalence
A payoff structure and its replication
Put-call parity, boxes, conversions
Settlement
Same asset, two settlement stages
Lagged assets in fast cycles at a priced bridge
30.1 Detection
Step
Method
Cost
Edge update
Recompute weight for affected edges only. Size-aware slippage from live depth
~2 µs per edge
Candidate index
Built at policy load: every cycle of length 2–4 the whitelist permits, indexed by edge and tagged with its path
offline
Incremental scan
On edge change, re-evaluate only indexed cycles containing that edge. Typically 20–200
~50 ns per cycle
Background sweep
Bellman-Ford / SPFA over the full graph to discover cycles the whitelist missed
low-priority thread
Profitability filter
Weight sum below a threshold including fees, slippage, bridge cost and a path-specific risk premium
inline
The candidate index is what makes this fast. Discovering unknown cycles is background work feeding the whitelist. Exploiting known cycles is a lookup.
30.2 Path Router
Edge composition
Assigned path
All conversion edges, one venue
1 — Intra-venue
Conversion plus transport, one region
2 — Cross-venue
Contains a mirror edge, both sides at target
3 — Mirrored inventory
Mirror edge, one side lacks inventory, hedge available locally
4 — Hedged bridging
Mirror edge, remote venue supports resting orders
5 — Passive anchoring
Mirror edge, remote venue quotes firm beyond round trip
6 — Firm-quote bridging
Contains basis edges
7 — Representation basis
Contains equivalence edges
8 — Payoff equivalence
Where several paths are eligible, the optimiser chooses. Path assignment is a policy decision made globally with full cost and risk information, not a local heuristic.
31. The Eight Execution Paths
Each is defined by what it does about coordination, because coordination is what latency makes expensive. New York to London is roughly 28 milliseconds each way; no cycle can span that atomically.
#
Path
Mechanism
Coordination
Latency
Primary risk
1
Intra-venue
Four orders back to back on one pre-warmed session
None — one session
2–6 ms
Book moves between first and last order
2
Cross-venue
Latency-equalised parallel dispatch from pinned I/O slots
None beyond one process
5–15 ms
Fill-time dispersion
3
Mirrored inventory
Asset held in both regions; each trades independently against a distributed reference, bounded by direction gating
None — policy in advance
2–6 ms per side
Reference staleness, drift
4
Hedged bridging
Execute locally, hedge immediately with a local derivative, complete remotely when convenient
Loose
seconds to minutes
Basis between hedge and exposure
5
Passive anchoring
Remote leg rests as a limit order, repriced locally from the reference
Pre-committed
local speed
Adverse selection, priced
6
Firm-quote bridging
Venue quotes firm for a window exceeding the round trip
From the venue
inside the window
Quote rejection, last look
7
Representation basis
Cycles between spot, perpetual, future, tokenized, ETF forms
None
5–15 ms to enter
Basis widening before convergence
8
Payoff equivalence
Options structures as cycles: parity, boxes, conversions
None — Greeks gate
5–20 ms to enter
Pin, assignment
31.1 Path 3 — The Cross-Region Solve
   SETUP    hold asset X in BOTH regions; distribute reference price R, threshold, targets
   EXECUTE  Region A: local price below R by > threshold -> BUYS locally
            Region B: local price above R by > threshold -> SELLS locally
   RESULT   global position unchanged; cash + spread - fees; inventory drifts, rebalanced later
Region state
Permitted direction
Inventory below target
May buy only
Inventory above target
May sell only
At target, inside band
Either direction, reduced size
Outside hard band
Reducing direction only
Direction gating makes same-side trading across regions impossible by construction rather than by coordination. A stale reference can cost one side&apos;s band, never both. Capital cost is roughly double, and that is what the optimiser is paying for.
31.2 Saga Semantics
   Proposed -> Reserved -> Firing -> Complete
                  |          +-> PartiallyFilled -> Resize | Unwind -> Closed
                  +-> Rejected                                 +-> Escalated
State
Rule
Reserved
One compare-and-swap across all legs, projected against the settlement timeline. All or nothing
Firing
Dispatched per the path&apos;s ordering and equalisation policy. Deadline timer starts
PartiallyFilled
At or above the minimum viable fraction and decomposable: resize and complete. Otherwise unwind
Unwinding
Compensating orders close residual exposure. Loss bounded by the priced-in premium
Escalated
Unwind failed or residual exceeds threshold. Halt the cycle class, alert, hold hedged
31.3 Leg Ordering
Policy
When
Trade-off
Simultaneous fire
Path 1, and Path 2 with deep fast venues
Lowest time exposure. Requires full capital at every venue
Riskiest leg first
Path 2 with one thin or slow venue
If the hard leg fails, nothing else fired. Costs one round trip
Hedged entry
Path 2 with two liquid and two illiquid legs
Residual is a known position, not naked exposure
Resting completion
Cross-region only
One leg rests. Completes on fill or is abandoned at a defined cost
31.4 The Cross-Class Hedge Map
Path 4 hedges a position with a locally available instrument, and divestment often needs to hedge before it can unwind cleanly. Both assume the platform knows what offsets what across asset classes. That knowledge is the hedge map, and it was previously assumed rather than specified.
Records
Used by
Which instrument hedges which exposure, and how well
Path 4 hedge selection
Hedge ratio and its stability across regimes
Sizing the hedge leg
Basis risk between the hedge and the exposure
The Path 4 gate check, and holding-period risk
Liquidity of the hedge instrument now
Whether the hedge can actually be placed at size
Cross-class hedges
An equity index against a basket, a perp against spot, a future against a bond
Degradation
When a hedge relationship weakens, surfaced from the causal graph
The hedge map is populated from the causal graph and from realised hedge performance, not asserted once and trusted. A hedge relationship that fails its conditions is retired the same way a causal edge is, because a hedge that no longer hedges is worse than none — it is a second position wearing the costume of a hedge.
32. Dispersion and Settlement
32.1 Fill-Time Dispersion
The dominant risk on any multi-venue execution. Pricing a premium against it is not solving it. Five mechanisms reduce it directly and compound.
   latency: A=3ms B=8ms C=5ms D=4ms
   NAIVE     send all at t=0        -> arrivals spread over 5 ms
   EQUALISED send at (max - own)    -> all arrive at 8 ms; dispersion collapses to jitter
Mechanism
Effect
Latency-equalised dispatch
Timer wheel on the dispatch thread and a rolling per-venue latency percentile. Five to tenfold reduction. The single largest effect
Passive-first on thin venues
Rest on the slow venue, cross the fast ones on its fill. Removes it from the exposure window
Size decomposition
A partial fill at or above the minimum viable fraction completes at reduced size rather than unwinding
Venue-native IOC and FOK
The venue enforces the guarantee. Cheapest control available
Dispersion-aware sizing
Bounds worst-case unwind cost rather than reducing its probability
Together these move the Path 2 completion target from roughly 85 percent to above 93 percent.
32.2 Settlement Stages and Bridges
Stage
Usable as funding?
Available — settled, unencumbered
Yes, immediately
Reserved — held against a cycle or live quote
No. This prevents double-spend
In-settlement — traded, not settled
Only via a bridge, at a priced cost
In-transit — moving under a corridor
No, and arrival is tracked
Encumbered — posted as collateral
No, unless rehypothecation is permitted
Committed — unfunded private commitment
No, and it is a hard reservation
Bridge
Mechanism
Cost basis
Venue margin
Trade on margin against unsettled proceeds
Financing rate per day
Broker credit line
Committed intraday credit
Facility fee plus drawn rate
Cross-margin
Settled assets collateralise the unsettled leg
Haircut on collateral
No bridge
Wait for settlement
Capital occupied for a day
Making the bridge an explicit priced edge is what lets lagged assets join fast cycles safely. Reservation is future-tense: availability is checked at the moment each leg needs it, from a small fixed-size timeline per venue-asset maintained incrementally, still one compare-and-swap.
33. The Unified Risk Gate
One gate over net intents and arbitrage cycles. It returns veto or silence, never approval. Silence is permission, which makes it a pure function, exhaustively testable, and fail-closed by construction — an error or timeout is the absence of silence.
   per-strategy checks -> before netting
   netting
   feasibility        -> can it execute at this size
   aggregate checks   -> on the net intent, O(1)
   cycle checks       -> whole cycle, atomically reserved
   path extensions    -> per assigned path
Check
Source
On failure
Strategy budget and caps
Cached grant, decremented locally
Drop that contributor from the net
Feasibility — minimum, tick, fee floor, gas, depth
Shipped per-venue constraints
Veto, and record the binding constraint
Settlement-aware reservation
Reservation table plus projected timeline
Veto
Instrument, class, venue, region, factor, family, causal-driver exposure
Incremental aggregates, O(1)
Veto
Effective breadth and crowding caps
Aggregates plus policy
Veto
Gross, net, leverage, drawdown breaker
Aggregates
Veto and halt
Belief freshness
Cached prior TTL
Reduce to conservative multiplier
Worst-case unwind cost, cycles
Live depth on every leg
Veto if it exceeds expected profit by the configured multiple
Venue health, latency stability, rate budget
Connectivity module
Veto
Kill switch and policy freshness
Cached flags with TTL
Veto all; flatten if stale past the hard limit
33.1 Path Extensions
Path
Additional check
3 — Mirrored
Direction permitted by inventory band. Reference inside TTL. Both, every time
4 — Hedged bridge
Hedge instrument available at depth, now, before the first leg
5 — Passive anchor
Resting order live. Adverse-selection premium covered by the spread
6 — Firm quote
Window exceeds round trip plus execution plus margin. Honour rate above threshold
7 — Basis
Expected carry exceeds financing plus capital-occupancy cost to convergence
8 — Payoff equivalence
Net delta, gamma, vega inside limits. European-style or explicit assignment budget
Market making
Inventory reserved behind the quote. Rate budget available. Toxic-flow state clear
Market creation
Valuation present with confidence. Bounded exposure not exceeded. Class approved
Every extension is additive and deterministic. The gate remains a pure function over cached policy and local state, which keeps it testable against fixtures and fast enough for the microsecond budget. Every verdict, including silence, is logged and counterfactually scored.
34. Venue Onboarding
The design names about twenty venues as a starting set. Reaching every exchange and every venue type is not a matter of listing more names; it is a matter of having a framework that makes adding the hundredth venue a repeat of adding the first. That framework is what turns a starting set into arbitrary breadth.
34.1 What an Adapter Must Provide
Provides
Why it is required
Protocol binding
WebSocket, FIX, REST, RPC or RFQ. The one thing that must be hand-written per venue
Fee schedule
Maker, taker, tiered, and any fixed floor. Feeds the feasibility gate and the profitability filter
Order-type capability matrix
Which of IOC, FOK, post-only, self-trade-prevention the venue supports. Feeds dispersion control
Settlement rules
Stage and cycle, joined to the settlement calendar for this venue&apos;s jurisdiction
Withdrawal policy
Allowlist mechanics, delays, and limits. Feeds the corridor and custody model
Rate limits
Order and message limits. Feeds the quote rate budget
Latency profile
Measured, not declared. Feeds latency-equalised dispatch
Minimums, tick and lot
Feeds the feasibility gate
Reconciliation endpoint
How balances are read, for the wallet
34.2 Venue Types
Most venues fit the order-book model the platform already has. Two do not, and naming them prevents treating a decentralised exchange as though it had a book.
Type
Model
Extra machinery
Central limit order book
Depth and price-time priority. The default
None beyond the adapter
Request for quote
A firm quote for a window. Path 6
Quote honour rate model, already present
Automated market maker (DeFi)
No book. Price from pool math; slippage from the curve
Pool-math slippage, block-time execution, MEV and contract risk — see 34.3
Auction
Periodic clearing, not continuous
Auction engine, bidding strategy, winner&apos;s curse
Dealer or OTC
Bilateral, negotiated
RFQ binding plus a counterparty credit view
34.3 Decentralised Venues Need Their Own Model
A decentralised exchange is not an order book behind a different protocol. Treating it as one is a category error the platform must not make.
Order-book assumption
DeFi reality
Slippage comes from depth
Slippage comes from pool math — constant-product or concentrated-liquidity curves
Execution is microsecond
Execution is block-time granular, and confirmation is probabilistic
Fills are private until placed
The mempool is public; every intent is visible, inviting front-running
The venue is a neutral matcher
There is MEV, sandwich risk, and reordering on every trade
Counterparty risk is the venue
Counterparty risk is the smart contract, on every position
A DeFi adapter therefore carries a pool-math slippage model, a block-time execution mode, an MEV and front-running risk estimate that the feasibility gate reads, and a contract-risk flag that the risk envelope treats as a distinct exposure. Where these are absent, the venue is registered as observe-only until they exist.
34.4 Promotion, Sim to Production
Stage
Gate
Registered
Adapter provides everything in 34.1. Venue type identified
Observed
Connected read-only. Book, fees and latency measured against the adapter&apos;s declarations
Simulated
Traded in sim against recorded and replayed data. Reconciliation verified
Shadow
Live connectivity, decisions logged, orders discarded and compared to expectation
Capped live
Real capital at a hard cap. Fill quality and reconciliation watched
Full
Cap raised as evidence accumulates. Enters the venue catalogue
A venue is never enabled from its own documentation alone. Declared fees, latency and order-type support are verified against measurement in the observed stage, because a venue that misdescribes itself is a venue that will surprise the platform with real capital at risk.
35. Position Lifecycle and Divestment
Investing was a first-class flow. Divestment was a side effect scattered across strategy sells, the liquidity ladder, breakers and the tax engine. That is a genuine gap: a position could be orphaned when its strategy retired, unwound arbitrarily when a user reduced an allocation, or held indefinitely past the thesis that opened it. This section makes divestment first-class.
35.1 A Position Has a Lifecycle
State
Meaning
Transition
Opened
A strategy took the position with a thesis and an intended horizon
On fill, linked to the strategy and the belief that supported it
Held
The thesis remains live and the strategy is funded
While the belief holds and the horizon has not elapsed
Flagged
The thesis weakened, the strategy is decaying, or the horizon passed
On decay detection, belief expiry, or horizon elapse
Unwinding
Being closed deliberately, tax-aware and cost-aware
Sized and ordered against the liquidity ladder
Orphaned
Its strategy retired but the position remains
Never left dangling — reassigned or unwound, see 35.2
Closed
Fully exited
Realised P&L attributed, tax lots settled, episode stored
35.2 The Three Questions Version 9 Left Open
Question
Answer
What happens to a retired strategy&apos;s open positions?
They are never orphaned silently. On retirement, each position is either reassigned to another funded strategy that shares its thesis, or scheduled for unwinding. A position with no owner is a reconciliation break, not a normal state
How is a reduced allocation unwound?
By an explicit policy, not arbitrarily: worst-thesis-first, then tax-aware lot selection, then cost-aware ordering against the liquidity ladder. Never a forced close at a bad price to meet a routine reduction
What catches a position held past its thesis?
A thesis-expiry sweep. A position whose strategy has no live signal and whose intended horizon has elapsed is flagged for review and, absent a reason to hold, unwound. Nothing is held by inertia
35.3 Unwind Ordering
When several positions must be reduced, the order is a decision, not an accident. It composes the liquidity ladder, the tax engine and the cost model into one ranking.
Priority
Rule
1
Positions whose thesis has failed, before those merely being trimmed
2
Highest tax efficiency — harvest losses, defer gains near a long-term boundary
3
Lowest cost to exit — top of the liquidity ladder first, per Section 25.4
4
Least disruption to remaining positions — do not break a hedge that still protects something held
5
Respect the cross-margin model — do not liquidate collateral that backs a retained position
A routine withdrawal or reduction is served from reserve first and from the top of the liquidity ladder second. It never forces a position closed at a bad price, and it never liquidates collateral out from under a position that is staying. Divestment done badly is how a good book becomes a loss on the way out.
36. Regions
Three regions, one binary. Every region runs the identical compiled artifact. What differs is which venues it connects to, which strategies its universes cover, and which mirrors it participates in. The architecture is the same at one region and at eight.
Region
GCP region
Machine
Venues
Asset classes
Mirror role
Americas
us-east4
c3-highcpu-22
Coinbase, Kraken, Gemini, CME, US equities via broker, prediction markets
Crypto spot and perp, futures, equities, prediction
BTC pair with Europe — first deployed
Europe
europe-west2
c3-highcpu-8
LSE, Euronext, LMAX, ICE Europe, Deribit, Bitstamp
Equities, FX, crypto spot and options, futures
BTC pair with Americas
Asia-Pacific
asia-northeast1
c3-highcpu-8
Binance, OKX, Bybit, BitFlyer, Upbit, TSE, SGX
Crypto spot and perp, equities, futures
Candidate for second pair
Each carries roughly 3,600 strategies with about 1,000 in the hot tier. Central intelligence and optimisation run once, in us-east4.
36.1 Identical Everywhere, Configured Per Region
Identical
Configured
Same build hash
Venue adapters — which sessions open
23 modules
Strategy plan — universes that are local
Core layout: 0–1 OS, 2–15 isolated
Whitelist — cycles reachable from here
One riskgate crate
Inventory targets, mirror bands, placement
Netting rules and crossing cap
Machine size by venue count and message rate
Retention classes from first connection
Credentials, region-scoped, IP-restricted at venue
Blue-green deployment, shadow mode first
Belief priors and episodic digest for local universes
36.2 What Arrives Versus What Only the Region Has
From central
Only local
Models scoring cost, probability, regime
Live order books, its venues, right now
Strategies, compiled, tiered, indexed
Its own measured latency per venue
Beliefs with confidence and TTL
Its inventory — available, reserved, settling
Episodic digest for the current neighbourhood
Its open orders and resting quotes
Causal digest — which edges are active
Fill quality drift, in real time
Budgets, whitelist, limits, targets, feasibility constraints
Local time before the opportunity closes
The decision is made where both halves exist, which is inside the node. A central decision would need the live book, and the live book is 28 milliseconds away.
36.3 Failure Isolation
Event
Affected
Everything else
One venue session drops
That venue quarantined; its cycles leave the whitelist
Other venues in the region continue
A region&apos;s node crashes
That region dark; reconciles against every venue before resuming
Others unaffected; mirrors involving it suspend
Whole cloud region fails
That region dark; global exposure recomputed without it
Others continue; redeploy to a second zone
Central unavailable
All regions on the last shipment; staleness clock starts
Investing continues; opportunity narrows in defined order
Reference price stale
Mirrored trading disabled everywhere
Local strategies and intra-venue cycles continue
Model degraded past bound
Strategies depending on it fall back or pause
Every other strategy unaffected
37. Treasury and Autonomous Capital Movement
Multi-venue and multi-region investing is impossible if capital requires a human to move. The control moves from the transaction to the corridor: humans approve, once and with a delay, where money is permitted to go. Machines decide, continuously and inside hard caps, when it goes.
   CORRIDOR (human-signed, rarely changed)
     source -> destination | asset | destination address [allowlisted]
     max per transfer | per hour | per day | cumulative | min interval | hours
   TRANSFER INTENT (machine-generated) must reduce deviation from optimiser target
   TRANSFER GATE (deterministic, veto-only) -> EXECUTED -> LEDGER -> RECONCILED
The blast radius is bounded by the allowlist, not by the correctness of the automation. A fully compromised transfer engine can only move money to destinations a human already signed, in amounts already capped. It cannot invent a destination.
37.1 Corridor Lifecycle
Stage
Actor
Control
Proposed
Optimiser or human
Full parameters and a stated purpose
Reviewed
Human
Destination verified out of band, directly with the institution
Signed
Human, hardware key
Signature covers destination and every cap
Time-delayed
System
24 hours before activation. A new destination cannot be used immediately
Active
System
Every transfer checked against the signed definition, not a cached copy
Suspended
Any anomaly, or any human
Instant. Reactivation needs approval; suspension does not
Revoked
Human
Permanent, immediate
37.2 Delay Applies to Destinations, Not Caps
Change
Control
Delay
Add a new destination
Human, hardware key, out-of-band verification
24 hours, mandatory
Raise a cap on a verified destination
Two human approvals, hardware key
None
Lower a cap, suspend, revoke
Any human, or any anomaly detector
None
Adjust interval or hours
One human approval
None
Money still cannot reach a destination no human verified 24 hours earlier. What was removed was a delay on operations that cannot change where money goes.
37.3 Transfer Gate
Check
On failure
Corridor active, signature valid, destination allowlisted
Veto and alert
Within per-transfer, hourly, daily, cumulative caps
Veto
Minimum interval elapsed
Veto
Reduces deviation from optimiser target
Veto — no transfer without a stated purpose
Source balance sufficient after reservations, in-flight settlement and commitments
Veto
Velocity breaker not tripped; anomaly detector clear
Veto all, alert
Kill switch state
Veto all
37.4 Custody
Asset type
Custody
Signing
Fiat at brokers and banks
Institution of record
API credentials scoped to corridor destinations, IP-restricted
Crypto in venue custody
Venue
Withdrawal allowlist at the venue, mirrored by the corridor
Crypto in self-custody
MPC threshold signing
Policy engine holds one share, released only on gate approval. No single component can sign
Collateral and margin
Venue
Managed as inventory, not transfers. Never leaves
Private commitments
Fund administrator
Capital calls paid from reserve through a signed corridor
Three independent enforcement points must agree before capital leaves a venue: the venue&apos;s own allowlist configured out of band, Algorik&apos;s signed corridor, and the custody policy engine holding a signing share. Trading authority and transfer authority never share an identity, a credential or a code path.
38. The Wallet
The platform&apos;s single view of every unit of capital it controls, wherever it sits, and the connection layer to every external system holding it. What it is not, and this is load-bearing: it is not a signing authority. The Wallet sees everything and moves nothing on its own.
38.1 Read and Write Are Separate Systems
Read path
Write path
Purpose
Balance and holding aggregation across every external system
Moving capital between venues, custodians, chains
Frequency
Continuous
Rare — only when inventory deviates from target
Credentials
Read-only API keys, watch-only addresses, view keys
Withdrawal-scoped credentials and MPC shares
Trust zone
Wallet — lower privilege, no signing capability
Treasury — the highest-privilege zone
Failure impact
Stale balances. No capital at risk
Unauthorised movement, bounded by allowlist and caps
Code path
Aggregator and adapters. Signing crate NOT linked
Transfer engine, gate, custody policy engine
The read path holds no key material capable of moving anything, enforced at the binary level and verified by dependency audit. A fully compromised read path leaks balances and cannot move a dollar.
38.2 Adapters
Adapter
Reads
Can initiate
Exchange
Spot, margin, futures balances; collateral; open orders; funding
Withdrawal to a venue-side allowlisted address only
Chain
Native and token balances at derived and watched addresses
Unsigned transaction construction only
Custodian
Holdings, pending settlements, transfer status
Transfer request into the custodian&apos;s own approval flow
Broker
Cash, margin, buying power, settled and unsettled
Internal transfer between accounts at the same broker
Bank
Balances, pending transfers, confirmations
Payment initiation into the bank&apos;s own approval flow
Fund administrator
Commitments, calls, distributions, NAV statements
Nothing. Statements reconcile against marks
Watch-only
Balance only
Nothing
Hardware
Public keys and addresses
Nothing autonomously. A human signs corridors with it
38.3 Reconciliation
   expected = ledger_balance - reserved + in_flight
   delta    = external_balance - expected
   |delta| < tolerance -> reconciled;  else HALT that venue-asset, alert, never auto-correct
Asset class
Tolerance
Crypto spot
Dust floor only. Instant settlement; any non-dust delta is a real break
Perpetual futures
One funding interval&apos;s accrual at the current rate
Dated futures and margin
One mark-to-market interval
Fiat at broker or bank
One day&apos;s interest accrual
Equity
Zero beyond dust. Unsettled positions are already in the timeline
Private positions
Statement cadence. A mark is compared to the administrator&apos;s NAV, and a divergence beyond the mark&apos;s own confidence band is flagged
Tolerance is a formula, not a constant. A persistent non-zero delta inside tolerance is a modelling defect and opens a ticket. An external balance exceeding expectation is treated with the same severity as a shortfall. The Wallet never writes a correction to the ledger.
38.4 Destination Registry
Stage
Where
Control
Discovered or proposed
Wallet
Added as a candidate. Can be watched; nothing can be sent to it
Verified out of band
Human
Confirmed directly with the institution, never from inside the platform
Signed into a corridor
Treasury, hardware key
Signature covers destination and caps
Time-delayed
System
24 hours before the corridor activates
Mirrored at the venue
Venue allowlist
The third independent enforcement point
Session-based connect-your-wallet protocols are not adopted. They introduce a third-party relay for a convenience that registered addresses and signed corridors already cover, and Algorik is the fund rather than a consumer application.
39. Decision Authority
Authority flows downward as constraint, never upward as request. Nothing on the hot path asks for permission — every permission it holds arrived in advance and is cached.
Layer
Authority
Can say yes
0 — Human
Approves families and thresholds, signs destinations, sets the mandate and capital ceiling, approves market creation, holds the kill switch
Yes — ultimate
1 — Hypothesis gate
Which causal claims and strategy classes may be tested at all
Yes — gates existence
2 — Strategy lifecycle
Promotes through statistical gates, retires on decay
Yes — within approved families
3 — Cognition
Forms beliefs, estimates causality, retrieves analogues, scores counterfactuals
No — informs only
4 — Valuation
Marks assets with method and confidence
No — informs only
5 — Capital engine
Deployed share, exploration budget, transfer sizing and routing
Yes
6 — Risk engine
The envelope at ten levels
No — constrains only
7 — Optimiser
Allocation, cycle selection, path assignment inside the envelope
Yes
8 — Strategy engine
Evaluates the compiled plan, produces intents
Yes — proposes
9 — Feasibility gate
Whether an intent can execute at this size
No — veto only
10 — Intent netting
Aggregates, crosses internally, attributes
No — consolidates only
11 — Risk gate
Evaluates net intents and whole cycles
No — veto only
12 — Leg coordinator
Ordering, equalisation, sizing within the approved intent
No — method only
13 — Transfer gate
Evaluates transfers against signed corridors
No — veto only
14 — Ledger
Records what happened with full attribution
No — records
39.1 Where AI, Quantum and Language Models Sit
Capability
Method
Authority
Family, regime and horizon allocation
Quantum plus Rust classical optimiser
Sets budgets inside the envelope
Cycle selection and path assignment
Quantum plus classical
Sets whitelists and routes
Capital and mirror placement
Quantum plus classical
Sets targets. Transfers still pass the corridor gate
Causal estimation
Inference over natural experiments and own flow
Constrains sizing, enables explanation
Belief formation
Bayesian updating over evidence
Scales size by confidence
Episodic retrieval
Approximate nearest neighbour
Informs belief
Counterfactual scoring
Shadow execution against recorded state
Recalibrates rules through the approval path only
Hypothesis proposal
Language model reading filings, news, the graph
Enters the statistical gate like any candidate
Fact extraction and explanation drafting
Language model
Facts carry provenance and confidence; drafts are rendered, never acted on
Cost, dispersion, regime estimation
ONNX models, shipped, in-process
Advisory input to deterministic filters
Quote pricing and skew
Deterministic formula with model and belief inputs
Method only, inside limits
Risk and transfer enforcement
Deterministic Rust
Absolute veto — no model involved
Market creation, family approval, destinations
Human with a hardware key
Absolute
No model has decision authority anywhere. Cognition changed what the platform knows and how confidently it acts; deterministic code still holds every veto. No language model touches a trade, a cycle or a transfer — latency and non-determinism disqualify it independently.
40. Experience, Identity and Explanation
Web and mobile are the control surface for capital. One Leptos codebase in Rust, shared types with every backend service, and an installable progressive web app rather than a native shell.
40.1 What a User Controls
Surface
Shows
Acts on
Portfolio
Value, allocation, performance and exposure per strategy and horizon. Marks carry method and confidence
Nothing — the ledger is truth
Wallet
Reconciled balances across venues, chains, custodians, brokers, banks. Available, reserved, settling, in transit, committed
Raise a transfer intent
Money movement
Deposit, withdraw, move between strategies and regions
All through the corridor path
Strategy marketplace
Families that cleared the gate, with evidence, capacity, and correlation against what is already held
Fund, set a maximum
Strategy review
Live against holdout band, capacity headroom, attributed P&L, crowding. Exception-first
Reduce, pause, retire
Explanation
What the platform believed, why, and which causal path supports it
Nothing — understanding
Private positions
Commitments, call schedule, expected distributions, mark and method
Commit; decline a call at stated consequence
Exploration
What the platform is spending to learn, and what it learned
Adjust the exploration share
Ten thousand strategies is not a list. The strategy view opens on exceptions — tens of rows — rolls up to 128 families, sorts by contribution, and treats the full list as a query result. The strategy count is the least interesting number in the platform; effective breadth, netting ratio and exception count describe its state.
40.2 Explanation
A user asks
What answers it
Why did you take this position?
The belief that supported it, its confidence, and the evidence that formed it
Why do you believe that?
The causal path through the graph, and the episodes that resemble now
Why this size?
Edge, volatility, grant, and the confidence multiplier, shown separately
Why not the obvious trade?
The gate that declined it and the counterfactual score of declining it
Why this strategy and not that one?
The optimisation run, the objective, and the correlation that ruled the other out
What do you not know here?
The self-model — stated coverage gaps and unreliable estimates
Is this platform sensible at my capital?
Total cost against attributed return, stated plainly
Attribution says which strategy caused a fill. Explanation says what the system believed and why. Only the second earns trust, and the last row is the one that most distinguishes a cognitive system from a confident one.
40.3 Identity and Access
Concern
Approach
Sign-in
Passkeys via WebAuthn platform authenticators. Hardware-backed, non-exportable. No passwords anywhere. TOTP fallback for a device that cannot register
Step-up
Authentication strength scales with consequence, verified per action
Devices
Registered, listed, individually revocable, with attestation
Mandate
Per-user capital, risk tolerance, permitted families, liquidity floor, exploration share, tax jurisdiction
Recovery
Identity verification plus a mandatory delay, every registered contact notified at the start
Recovery bound
A recovered account still needs a hardware key and the full 24 hours to add a destination. A fraudulent recovery can see balances and halt trading; it cannot send capital anywhere not already verified
Roles
Owner: everything. Operator: monitor, halt, tighten, acknowledge. Viewer: read only. Auditor: every ledger entry, verdict and approval, including silences
Audit
Every authenticated action recorded with actor, surface and authentication strength
40.4 Web and Mobile
Action
Web
Mobile
Authentication
View everything
yes
yes
session
Halt — pause, flatten, kill switch
yes
yes
biometric
Move capital between strategies
yes
yes
session + confirm
Withdraw to a verified destination
yes
yes
biometric
Second approval on a cap change
yes
yes
biometric + enclave key
Fund a new strategy family
yes
no
session + confirm
Commit to a private position
yes
no
hardware key
Register and sign a destination
yes
no
hardware key + 24 h
Loosen a risk limit
yes
no
hardware key + second approval
Approve market creation
yes
no
hardware key
Place a manual trade
no
no
no path exists
Mobile ships as an installable Leptos PWA. Web Push and WebAuthn cover the two capabilities that justified going native. A push reliability drill — delivery above 99.5 percent, p99 under 15 seconds over 30 days — is the Phase 13 exit criterion. The native shell is built only if that drill fails. Stopping is easy and available everywhere; starting, loosening, committing capital for years, or sending money somewhere new is harder and available in fewer places.
40.5 Experience Architecture
The customer-facing product is one system with three surfaces — public landing, authenticated portal, and mobile — over one application plane, behind one security edge. It is a control surface for capital, not a trading terminal. There is no path from any surface to a strategy, an order, a venue, a QPU, a signing key, or the ledger itself; every surface reads through application APIs and raises intents that enter the same workflows a machine-originated intent would.
Layer
Contains
Provider
Experience
Landing, portal, admin views, installable PWA. One Leptos codebase, shared types
Rust
Public edge
Cloud Armor, Global HTTPS Load Balancer, Cloud CDN for the static shell, identity boundary
Google
Application APIs
portal-api, account-api, portfolio-api, investment-api, wallet-api, treasury-api facade, strategy-api, research-api, admin-api, entitlement-service
Rust on Cloud Run
Investment platform
The seven planes, unchanged
Rust, Google, IBM
Customer traffic and trading traffic never share a load balancer, an identity, a credential or a route. Customer traffic enters at the public edge and terminates in an application API; trading traffic leaves a regional node through Cloud NAT to a venue. Venue connectivity is never behind the public edge.
40.6 Public Website
The landing experience explains the platform and admits the user. It carries no performance claims and no account data. Every quantitative statement carries a status — architecture, target, demo or measured — and nothing is measured before the Phase 3 gate. Sign-up creates an identity with a passkey and an empty mandate; nothing is investable until capital is added and a mandate is set.
40.7 Investor Portal
Eight areas — Overview, Invest, Capital, Intelligence, Risk, Activity, Platform, Account — plus role-gated Admin. Every screen is a read of the ledger or a raise of an intent. Investment requests enter the capital engine; transfer intents enter the transfer engine; reductions, pauses and retirements enter the strategy lifecycle. The portal shows verdicts as they happen, including vetoes with the rule that fired, and can explain any position by traversing the attribution chain in Section 43.4.
Area
Screens
Raises
Overview
Dashboard
Nothing
Invest
Invest, Strategies, Opportunities, Portfolio, Positions, Private positions
Investment request; reduce, pause, retire; commit
Capital
Wallet, Add capital, Withdraw, Move capital, Funding sources, Destinations
Expected inflow; transfer intent; destination proposal
Intelligence
AI intelligence, Market intelligence, Explanations, Research
Nothing
Risk
Risk overview, Exposure, Limits, Allocation
Tighten only; loosening is Admin with a hardware key
Activity
One faceted ledger of orders, fills, transfers and approvals; execution trace; statements and documents
Nothing
Platform
Global platform, Regional status, System health
Halt
Account
Profile, Security, Mandate, Notifications, Preferences
Mandate change under step-up
Admin
Strategy families, Capital policy, Corridors, Risk policy, Platform operations, Users and roles
Approvals under hardware key
40.8 Mobile Experience
The same codebase, installed. Five primary areas — Home, Invest, Portfolio, Wallet, Activity — with the halt control on every screen. Mobile consumes the same application APIs with the same session model; there is no separate mobile backend. Capabilities absent on mobile — new destination, private commitment, family approval, limit loosening — are the ones that require a hardware key, per 40.4, and are presented as web-only rather than missing.
40.9 Application and API Layer
Application services are interface services around the existing domains, not replacements for them. They hold no financial state. Each is scoped to one domain, authorises server-side against the user's role and entitlements, and emits an audit event for every authenticated action with actor, surface and authentication strength.
Service
Reads
Raises
Never
portal-api
Composes screens from the services below
—
Reaches a node
account-api
Identity, devices, mandate, entitlements
Mandate change, device registration
Custody material
portfolio-api
Positions, marks with method and confidence, attribution, performance
—
A mark
investment-api
Approved families, capacity, correlation with holdings
Investment request
An order
wallet-api
Reconciled balances, destinations, transfer status
Expected inflow
A transfer
treasury-api
Corridors, caps, gate history
Transfer intent
A signature, a gate bypass
strategy-api
Family state, exceptions, live against holdout
Reduce, pause, retire
Funding beyond capacity
research-api
Explanation, beliefs, episodes, counterfactuals, self-model
—
Anything
admin-api
Policy, corridors, families, platform health
Approvals under hardware key
A loosening without second approval
40.10 Wallet Experience
The Wallet page is a presentation of Section 38: total balance, the five balance states — available, invested, reserved, in settlement, in transit — with committed shown when nonzero, per-asset and per-location holdings with reconciliation status, pending transfers with corridor and gate state, and the portfolio's reserve requirement against what is held. It holds no state. A reconciliation break is displayed as a halt on that venue-asset, never as a number the interface has corrected.
40.11 Investment Experience
The investable primitive is the strategy family that cleared the gate. Named portfolios — Balanced, Growth, Market Neutral, Global Macro, Quantitative Alpha, Opportunistic, Custom — are fixed-weight bundles of families and carry no ledger identity of their own; attribution remains per family. An investment request states family weights and an amount within available capital, capacity and the mandate; the capital engine funds it, possibly partially, at the next allocation run. The interface shows live performance only against the holdout band and marks simulated performance as such. It never shows a projected return.
Step
What the user sees
What the platform does
Select
A family or bundle with objective, composition, risk class, horizon, regions, capacity, correlation with holdings
Reads investment-api
Allocation
Family weights; ineligible families disabled with the reason
Mandate and entitlement check
Amount
Maximum is available capital, capacity and mandate ceiling; reserve after
Capital engine reserve rule
Risk review
Envelope utilisation before and after at ten levels; causal-driver overlap; exploration share
Risk policy, read only
Review and confirm
Cost against attributed return, after-tax note; session plus confirm
Creates InvestmentRequest
Funded
Pending, then partially or fully funded with grants shipped
Capital engine at next run; optimiser
40.12 Capital Movement
Deposits create an expected inflow that the wallet read path detects and reconciliation posts; the deposit address comes from the destination registry. Moves and withdrawals create transfer intents that pass the corridor gate in Section 37.3 unchanged; the interface shows each check and its result, and a veto is shown in plain language with the rule that fired. Destinations are added only through the registry in Section 38.4 with a hardware key and the 24-hour delay. Withdrawal requires biometric authentication on every surface and a second approval above a mandate-set threshold. A withdrawal is served from reserve first and the top of the liquidity ladder second, never by forcing a position closed.
Flow
Path
Authentication
Add capital
Funding source → destination → amount → review → instruction → expected inflow → detected → settled → available
Session plus confirm; biometric above deposit velocity or for a new source
Move capital
Source → corridor-reachable destination → asset → amount → stated purpose → transfer intent → gate → executed → ledger → reconciled
Session plus confirm; biometric on mobile
Withdraw
Approved destination → amount within corridor and daily caps → review → transfer intent → gate → executed → settled
Biometric; second approval above threshold
New destination
Proposed → verified out of band → signed → 24-hour delay → active → mirrored at venue
Hardware key, web only
40.13 Entitlement Model
Capabilities are granted per account from jurisdiction, product eligibility, role and mandate, and are evaluated server-side on every request. The interface hides a capability the account cannot have, disables one it could have but has not yet enabled, and states the reason in either case. No regulated capability is universally available. Which capabilities may be offered to which account types in which jurisdictions is a compliance determination, not a design one.
Capability
Gates
can_view_portfolio
Portfolio, positions, activity
can_invest
Invest, fund a family, set a maximum
can_deposit, can_withdraw, can_transfer
Add capital, withdraw, move capital
can_use_crypto, can_use_derivatives, can_use_private
Asset-class-scoped families and destinations
can_access_strategy:family
Per-family eligibility
can_view_execution_trace
The netting and gate trace on a fill
can_access_admin, can_approve, can_halt
Admin areas, hardware-key approvals, the halt control
40.14 Frontend Security Boundary
Browser and mobile reach only the public edge. Sessions are bearer tokens issued after passkey verification, bound to a device, with step-up verified per action. Application services run with workload identity and least privilege behind Private Service Connect. No credential, key, deposit private key or signing share is ever delivered to a client, and no client persists anything beyond a session identifier. Cloud Armor enforces rate limits and geographic policy; secure headers and CSRF protection are applied at the edge and in the application. The execution node, the ledger, the control fabric, IBM endpoints and custody services are unreachable from any client by network policy, not by convention.
A client may reach
A client may never reach
Cloud Armor and the Global HTTPS Load Balancer
Cloud Spanner
The static shell via Cloud CDN
Pub/Sub
Application APIs, with a session
algorik-node in any region
Nothing else
A venue, an exchange, a chain endpoint
IBM Quantum
Custody signing services or key material
41. Application Architecture
41.1 Workspace
  algorik/crates/
    types/ money/ sketch/ telemetry/ gcp/            shared foundations
    ingest/ entity/                                  ingestion
    worldmodel/ causal/ episodic/ belief/            cognition
    counterfactual/ selfmodel/ hypothesis/
    termstruct/ credit/ volsurface/ illiquid/        valuation
    cashflow/ corpactions/
    features/ stratcomp/ stratexec/ netting/         execution  [node + backtest]
    quoting/ graph/ cyclescan/ pathrouter/
    inventory/ mirror/ riskagg/ riskgate/
    feasibility/ legcoord/ greeks/
    qubo/ qasm/ ibmq/                                optimisation
    stats/ dataref/ taxlots/                         platform
    discovery/ registry/ lifecycle/ hedgemap/        breadth  [ingestion + execution]
    ui/ pwa/                                         experience
  algorik/bins/  algorik-node/  <~70 services>/
The backtest runner links stratcomp, stratexec, netting, quoting, graph, riskagg, riskgate, feasibility and belief verbatim. Backtest and live cannot diverge because they are the same code, and a backtest experiences the same confidence weighting and size rejections production will.
41.2 The Execution Node — One Binary, Twenty-Three Modules
Module
Responsibility
Connectivity
Venue sessions, RFQ, quote streams, per-venue latency and rate budget
Normalizer
Venue format to canonical event across representations
Book engine
Incremental depth per instrument per venue
Feature engine
Dirty tracking, incremental recompute, dependency index
Statistics engine
Streaming estimators and event-anchored snapshots, checkpointed to central
Inference
Shipped ONNX models via tract, in process, parallel to evaluation
Belief cache
Priors from central with a TTL. Drives confidence-weighted sizing
Episodic digest
Compact outcome distribution for the current neighbourhood
Strategy executor
The compiled plan, tiers 0 and 1
Quote engine
Two-sided quoting, skew, adverse selection, rate budget
Graph engine
Six edge classes over asset, venue, representation, settlement
Cycle scanner
Indexed detection plus background sweep
Path router
Assigns one of eight mechanisms from shipped policy
Feasibility gate
Minimum, tick, fee floor, gas, depth, withdrawal economics
Mirror manager
Direction gating, reference TTL
Inventory manager
Stages, settlement timeline, atomic reservation
Risk aggregator
Incremental counters including causal-driver exposure
Greeks engine
Delta, gamma, vega for options paths
Risk gate
Whole cycle or net intent against the shipped envelope
Intent netting
Aggregation, internal crossing, contributor vectors
Leg coordinator
Saga, latency-equalised dispatch, compensation
Execution
Order state machine, routing, resting orders, quote lifecycle
Adversary monitor
Fill-quality drift, pattern leakage, fingerprint randomisation
41.3 Thread and Core Assignment
Cores
Threads
Rationale
0–1
OS, telemetry drainer, control-plane client, plan compiler (background)
Never on an isolated core
2–5
Venue I/O — one busy-poll receive slot per venue group
Isolated. Parallel fan-out for equalised dispatch
6–7
Book engine, feature engine, statistics engine
Isolated. Feeds everything downstream
8–9
Strategy executor, tiers 0 and 1, with belief and feasibility
Isolated. Largest consumer of hot-path budget
10
Graph engine, cycle scanner, path router
Isolated
11
Quote engine
Isolated. Its own loop, its own rate budget
12
Intent netting, risk aggregator, risk gate
Isolated. Everything converges here
13
Leg coordinator, execution dispatch
Isolated. Owns the timer wheel
14
Inference via tract, adversary monitor
Isolated
15
Tier 2 strategies, background graph sweep
Isolated but yields
41.4 Node Configuration
Setting
Value
Machine
c3-highcpu-8 to -22 by venue count. Titanium offload, gVNIC TIER_1, compact placement
OS
Minimal Debian, no container runtime. Nothing between the binary and the kernel
Isolation
isolcpus for cores 2–15. Governor performance, C-states disabled
Memory
Huge pages preallocated, mlockall. No swap
Supervision
systemd with watchdog, Restart=always. One unit
Network
No external IP. Cloud NAT for egress, Private Service Connect for GCP services
Deployment
Immutable image, blue-green instance replacement, shadow mode before taking sessions
41.5 The Shipping Payload
#
Shipped
Cadence
1
Trained models — ten ONNX artifacts, signed
on promotion
2
Compiled strategy plan with belief and feasibility bindings
on change
3
Belief priors with TTL
seconds to minutes
4
Episodic digest for the current neighbourhood
minutes
5
Causal graph digest — which edges are active
on re-estimation
6
Regime state and its confidence
on change
7
Family budgets and capital grants
hourly, adaptive
8
Cycle whitelist and path assignments
1–5 min
9
Risk envelope at ten levels
30 s – 5 min
10
Inventory targets, mirror bands, reference price
fast clock
11
Feasibility constraints per venue
on change
12
Adversary profiles per venue
hourly
Signed at source, verified before swap, applied by atomic pointer swap without pausing trading. Stale items narrow the region in the order defined in Section 6.2.
41.6 Service Catalog
Plane
Services
Runtime
Ingestion
source adapters, deep-web adapters (query, api, registered, licensed, rendered, bulk), source-access-manager, deduplicator, extractor, entity-resolver, source-scorer, discovery-crawler, source-classifier, freshness-tracker
Cloud Run + Jobs
Cognition
world-model, causal-estimator, episodic-store, episodic-retriever, belief-engine, counterfactual-scorer, self-model, hypothesis-generator, explanation-builder
Cloud Run + Jobs
Valuation
term-structure, credit-engine, vol-surface, illiquid-valuer, cashflow-forecaster, commitment-tracker, corp-actions
Cloud Run + Jobs
Intelligence
strategy-generator, backtest-runner, statistical-validator, capacity-estimator, promotion-controller, canary-monitor, decay-detector, strategy-registry, strategy-compiler, feature-pipeline, trainer, eval-harness, model-promoter, meta-learner, adversary-modeller, market-simulator, risk-policy
Jobs + spot GPU
Optimisation
family-clusterer, regime-conditioner, problem-builder, classical-solver, quantum-solver, routing-gate, solution-scorer, policy-emitter, decomposition-coordinator, cadence-controller, advantage-tracker, scenario-engine
Cloud Run + Jobs + IBM
Capital and risk
capital-allocator, portfolio-engine, breadth-monitor, transfer-planner, liquidity-ladder, exploration-budgeter, crowding-monitor
Cloud Run
Ledger and treasury
reconciler, attribution-service, tax-engine, corridor-registry, transfer-engine, transfer-gate, custody-policy-engine
Cloud Run
Wallet and inventory
wallet-aggregator, wallet-adapters, wallet-reconciler, destination-registry, inventory-aggregator, settlement-tracker, mirror-coordinator, reference-price
Cloud Run
Registries and lifecycle
asset-class-registry, venue-onboarding, settlement-calendar, hedge-map, cross-margin-model, position-lifecycle
Cloud Run
Experience and identity
portal-api, portal-web, portal-pwa, account-api, portfolio-api, investment-api, wallet-api, treasury-api, strategy-api, research-api, admin-api, entitlement-service, auth-service, device-registry, stepup-service, mandate-service, recovery-service, approval-service, notification-service
Cloud Run + CDN
Data and observability
dataref-registry, dataref-resolver, cache-reaper, graph-builder
Cloud Run
Control fabric
policy-distributor, outcome-collector
Cloud Run + Pub/Sub
Execution
algorik-node × 3
GCE C3, systemd
Roughly seventy binaries, one language, one workspace. Everything except the execution node scales to zero.
42. The Rust Stack
Crate selection is opinionated so that implementers, human or agent, do not have to choose.
Function
Crate
Note
Money arithmetic
rust_decimal
Fixed point. Floating point for money is a review rejection
Lock-free channels
crossbeam
SPSC between pinned threads. No mutexes in the decision loop
Arena allocation
bumpalo
Hot-path memory allocated at start, reset per cycle
CPU pinning
core_affinity
Thread to isolated core
Async I/O
io-uring, rustix
Zero-syscall telemetry drain and archive writes
Connectivity management
tokio, control threads only
Busy-poll receive uses raw sockets
Dispatch scheduling
hand-rolled timer wheel
Latency-equalised release times
Plan representation
hand-rolled packed arrays
Cache-line aligned, evaluation-ordered
Bitmaps
roaring or fixed bitsets
Universe membership
ONNX inference
tract
Pure Rust, no FFI
Graph algorithms
petgraph offline, hand-rolled hot
Candidate index, world model traversal, decomposition
Vector retrieval
hand-rolled HNSW or equivalent
Episodic nearest neighbour, warm path
Causal inference
hand-rolled + nalgebra
Structural estimation, natural-experiment analysis
Options pricing
hand-rolled
Black-Scholes and Greeks
Term structure and credit
hand-rolled + nalgebra
Curve fitting, discounting, hazard rates
Statistics
hand-rolled + statrs
Deflated Sharpe, purged CV, trial accounting, calibration
Streaming estimators
hand-rolled
Welford, EW covariance, t-digest, reservoir
Clustering
linfa
Family construction by return correlation
QUBO heuristics
hand-rolled + rayon
Annealing, parallel tempering, tabu
Parameter optimisation
argmin
SPSA, COBYLA for the QAOA loop
Linear algebra
nalgebra
Hamiltonians, portfolio, factor decomposition
HTTP and gRPC
axum, tonic
Services; tonic as fallback for any GCP API lacking a crate
GCP clients
google-cloud-rust
Spanner, Pub/Sub, Storage, BigQuery
Dataframes
polars, arrow-rs, parquet
Replaces pandas. BigQuery interchange
ML training
burn, linfa
Adequate for Algorik&apos;s model sizes
ONNX emission
prost
Direct protobuf construction
Text extraction
hand-rolled + hosted model via reqwest
Facts with provenance and confidence
Content hashing
blake3
Data reference hashes
Tracing
opentelemetry, tracing
To Cloud Trace and Managed Prometheus
Signing
ring
Corridors, artifacts, post-quantum algorithms
Frontend
leptos, plotters
SSR with WASM hydration. Server-side SVG for the graph
Mobile
leptos PWA
Service worker, Web Push, WebAuthn. No native shell unless the drill fails
43. Canonical Data Model
Every object, with its owning plane. Each has one owner and one lifecycle, and all are defined once in the types crate.
43.1 World and Cognition
Object
Definition
Entity
A company, person, sovereign, commodity, contract, venue, chain, index or event, with a stable identifier
EntityRelation
A directed relationship: owns, supplies, competes, depends on, is collateral for, settles
WorldEvent
An observed occurrence linked to entities, with source, event time, receipt time and confidence
ResolutionSource
The authority that determines an event contract&apos;s outcome, with reliability history
CausalEdge
A directed causal claim with strength, lag, sign, evidence, confidence, and conditions
CausalGraph
A versioned snapshot of active edges, shipped as a digest
Episode
Compressed state, regime, causal context, beliefs, actions, outcome and surprise
Belief
A distribution over a proposition with evidence, causal path, confidence and TTL
Counterfactual
A declined path, shadow-executed and scored, attributed to the rule that declined it
Hypothesis
A causal claim with mechanism, prediction, falsifier, evidence and status
CapabilityEstimate
Where the platform is reliable, its coverage gaps, and its calibration
Regime
A named market state with entry and exit conditions and a confidence
SourceCandidate
A location the discovery crawler found, with tier, category, access class, sample quality, and a lawfulness verdict. Registered as a feed or rejected with a reason
DeepWebAdapter
Access mode, credentials reference, terms record, query plan, extraction schema, entity types, and measured freshness for a registered deep web source
43.2 Valuation
Object
Definition
YieldCurve
Term structure snapshot with discount factors and forwards
CreditProfile
Default probability, recovery, spread decomposition, covenant state
VolSurface
Full surface with skew and term structure
Valuation
A mark with method, inputs, confidence, as-of and next-review
Commitment
An unfunded capital promise with an expected call schedule and a hard obligation
CapitalCall
A drawdown demand with date, amount and consequence of failure
CashflowForecast
Probability-weighted projection of irregular contingent streams
CorporateAction
A split, dividend, merger, spinoff, right or delisting, with its position effect
LogisticsCost
Shipping, customs, storage, spoilage and marketplace fees for physical arbitrage
AuctionState
Bidding context, reserve estimate and winner&apos;s-curse adjustment
43.3 Execution, Capital and Ledger
Object
Definition
Instrument / Venue
A tradeable thing with stable identity; a trading location with fees, settlement rules, order-type capability matrix
Representation
The form an underlying takes: spot, perpetual, dated future, tokenized, ETF, synthetic, cash at T
MarketEvent
Normalised tick, quote, trade, book delta or firm quote with venue and receipt timestamps
GraphNode / GraphEdge
(asset, venue, representation, settlement) and the six edge classes with weight and size limit
Cycle / Path / CycleClass / Leg
Ordered edges returning to start with expected profit and worst-case unwind; the assigned mechanism; the approved template; one edge instantiated as an order
Strategy / StrategyFamily / StrategyPlan
Parameterised instance; correlation cluster with approved template and thresholds; compiled evaluation plan
Intent / NetIntent / InternalCross
Proposal before netting; aggregate with contributor vector; internally matched pair at reference mid
Feasibility
Whether an intent can execute at size, with the binding constraint recorded
Quote
Live two-sided quote with fair value, spread components, skew, size and reserved inventory
Reservation / InventoryPosition
Atomic hold across legs projected against the settlement timeline; asset at a venue split by stage
Mirror / SettlementBridge
Cross-region pairing with targets, bands, drift and reference; financing facility with cost and capacity
ExposureAggregate
Incremental counters by instrument, class, venue, region, factor, family and causal driver
RiskVerdict
Veto with reason, or silence. Always logged, always counterfactually scored
Order / Fill / Position / CashBalance
Instruction to a venue; execution carrying its contributor vector; net holding; currency at a venue
TaxLot
A position lot with acquisition date, basis, jurisdiction and holding-period state
CapitalGrant
Bounded, expiring permission per family per region
ExplorationProbe
Capital allocated to information gain, with the uncertainty targeted and what it resolved
Corridor / TransferIntent / Transfer
Signed route with caps and delay; machine proposal with stated purpose; executed movement
WalletAccount / Holding / Destination
External holding location; reconciled balance with delta; registered place capital may go, inert until signed
Mandate
Per-user capital, risk tolerance, permitted families, liquidity floor, exploration share, jurisdiction
ApprovalRequest
A pending action with the surface it may be approved from and the strength required
Explanation
What the platform believed, why, and the causal path supporting a decision
OptimizationRun
Instance, classical solution, routing decision, quantum solution, scores, winner, delta
Trial / CapacityEstimate
One backtest counted against a family&apos;s budget; capital level at which an edge decays
SufficientStatistic / EventSnapshot / DataReference / ResearchCampaign
Streaming estimator with error bound; book state at an own action; manifest with content hash; scoped research run with TTL cache
StrategyVersion / ModelVersion
Immutable approved artifacts with evaluation records and signatures
AssetClass
Registry record: valuation engine, settlement convention, tick and lot, calendar, margin regime, eligible families, hedge instruments
VenueProfile
Onboarding record: protocol, fees, order-type matrix, settlement, withdrawal policy, rate limits, latency, promotion stage
SettlementCalendar
Per-market sessions, holidays, settlement cycles, roll and expiry dates
HedgeRelation
What offsets what across classes, with ratio, basis risk, liquidity and degradation state
CollateralGraph
What collateralises what, with haircuts, per margin regime
PositionLifecycle
A position&apos;s state from opened through held, flagged, unwinding, orphaned, to closed, with its thesis and horizon
InvestmentRequest
A user-originated request to fund family weights with an amount inside available capital, capacity and the mandate. States: Pending, Partially funded, Funded, Declined. Funded by the capital engine at the next allocation run; carries no order
Entitlement
A capability granted to an account from jurisdiction, product eligibility, role and mandate, evaluated server-side on every request
ExpectedInflow
A deposit the user says they have sent, matched by the wallet read path and posted by reconciliation. Never available until the ledger says so
43.4 The Attribution Chain
  Fill -> contributor vector -> [Strategy, pro-rata] -> StrategyFamily -> Mandate -> User
       -> NetIntent -> [Intent...] -> Belief -> CausalEdge -> WorldEvent -> Entity
       -> Feasibility   what constraint nearly stopped it
       -> RiskVerdict   and its counterfactual score
       -> Cycle -> Path -> CycleClass       [arbitrage]
       -> Quote                              [market making]
       -> Episode       what the world looked like, and what followed
       -> TaxLot        basis, holding period, jurisdiction
       -> CapitalGrant -> OptimizationRun    which solver set the budget
       -> Explanation   renderable in plain language
The chain reaches from a fill to the world event that caused the belief that supported the strategy that produced it. That traversal is what makes explanation possible.
44. Storage
Class
Store
Holds
Grows with
Ephemeral
Process memory
Shipped models, plan, beliefs, digests, book state, graph, quotes, counters, reservations
Nothing — fixed capacity
Operational
Cloud Spanner
Ledger with contributor vectors, positions per user and strategy, tax lots, inventory, mirrors, corridors, mandates, entities, relations, causal edges, beliefs, commitments, valuations
Own activity
Episodic
Spanner + vector index
Compressed episodes indexed for retrieval
Elapsed time, few GB per year
Analytical
BigQuery
Per-strategy returns, dispersion, trial records, counterfactual scores, solver deltas
Strategy count and elapsed time
Objects
Cloud Storage
Plans, model artifacts, statistics checkpoints, event snapshots on a 90-day roll, bar fallback, audit under retention lock
Own order rate, rolling
Research cache
Cloud Storage, TTL
Fetched extracts for one campaign
Deleted on expiry
Warm cache
Memorystore
Portfolio snapshots, policy pending distribution
Nothing
External
Referenced
Market history, filings, registries — held by their owners
Not ours to grow
44.1 Deliberately Not Built
Not used
Reason
A time-series database
BigQuery handles analysis; the hot path needs memory. A third tier serves neither
A feature store
Features computed once in shared crates. A store adds a component without removing an implementation
A graph database
The world model is node and edge tables in Spanner. Traversal depth is shallow
A message broker on the hot path
One process needs no broker
Vertex AI or a model registry product
Versioning, lineage and promotion are rows and objects
A raw market history archive
The data policy forbids it. Referenced by manifest instead
44.2 Bounded State
Structure
Bound
Compiled plan
Hot-tier count capped at 1,200. A plan exceeding budget fails to compile
Intent buffer
Pre-allocated to maximum firing rate. Overflow vetoes, never allocates
Order book, graph, reservation, timeline, quote table
Fixed capacity with explicit eviction
Sufficient statistics
Fixed by estimator type. None grows with observations
Raw tick ring
Bounded in seconds of wall time
Event snapshots
Rolling window by age
Research cache
TTL per campaign. Expiry enforced
Belief and episodic digests
Fixed by shipped payload size
Telemetry ring
Fixed byte capacity. Oldest dropped, counter alerted
Every one is a fixed-capacity type. An unbounded collection in hot-path code is a review rejection, and in Rust the capacity is visible in the type signature.
45. Google Cloud and IBM
45.1 Google Cloud
Service
Carries
Phase
Compute Engine C3 / C3D
Execution nodes. Titanium offload, gVNIC TIER_1, compact placement, isolated cores
3
Compute Engine spot GPU
Model training, causal estimation, market simulation
2
Cloud Run and Cloud Run Jobs
Every non-node service across all seven planes
0
Cloud Spanner
Ledger, world model, causal graph, beliefs, episodes, mandates, valuations, commitments
3
BigQuery
Derived series, counterfactual scores, trial records, solver comparison. No raw history
1
Cloud Storage
Plans, models, checkpoints, snapshots, research cache, audit with retention lock
0
Memorystore (Valkey)
Warm cache. Never read from the hot path
3
Pub/Sub
Ships the twelve-item payload outward; carries outcomes and observations back
1
VPC (global)
One VPC across all regions. No peering, no gateway, no overlay
0
Cloud NAT, Private Service Connect
Controlled egress; private access to GCP services
0
Secret Manager, KMS, Cloud HSM
Credentials, signing keys, custody key material
0 / 5
Workload Identity Federation
Keyless identity for build and runtime
0
Artifact Registry, Cloud Build, Cloud Deploy
Build, test, scan, attest, roll out
0 / 1
Binary Authorization
Only attested images run
1
Cloud Armor, Global LB, CDN
Web and mobile only. Never in front of venue connectivity
1
Cloud Trace, Logging, Managed Prometheus
Observability fed by OpenTelemetry from Rust
1
Security Command Center
Posture and threat findings
1
Cloud Workflows
Lifecycle and ingestion orchestration where Pub/Sub is too loose
2
Firebase Cloud Messaging
Push for the installable PWA
13
45.2 IBM
Service
Carries
Reached by
Phase
IBM Quantum Platform
QPU access. Open plan through Phase 15; Flex once the routing gate shows a material gap on more than thirty percent of instances
REST
2
Qiskit Runtime
Sampler and Estimator with error mitigation and sessions
REST, QASM 3
2
Transpiler Service
Logical QASM to backend ISA
REST
2
Nighthawk QPU
Primary target. 120 qubits, square lattice, 5,000 two-qubit gates
via Runtime
2
Heron r2 / r3
Secondary at 156 qubits
via Runtime
2
Quantum Advantage Tracker
External validation reference
Reference
3
Quantum Safe
Post-quantum algorithms for corridor signatures and custody keys
Rust libraries
1
IBM is reached exclusively over HTTPS from Cloud Run jobs in the Optimisation zone — the only zone with external egress beyond ingestion reads, and its allowlist contains IBM endpoints and nothing else.
45.3 The C3 Trade-off
Consideration
Assessment
Penalty versus bare metal with kernel bypass
Roughly 30–80 µs on the receive path. C3 uses Titanium hardware offload, the architectural equivalent
Impact on Path 1 at 2–6 ms
Around 1–2 percent. The venue round trip dominates by two orders of magnitude
When it becomes binding
Only for top-of-book market making or pure latency arbitrage. Neither is in scope
Trigger to revisit
More than twenty percent of profitable cycles lost to speed rather than to selection, allocation or placement
46. Security
46.1 Trust Zones
Zone
Contains
May reach
Public edge
CDN, load balancer, Cloud Armor
Application
Application and identity
portal-api, auth, devices, step-up, mandate, recovery
Ledger — read. Capital engine, Treasury and Lifecycle — raise intents only. Never a node, a venue, a QPU or a key
Ingestion
Source adapters, extraction, entity resolution
External sources — read only. Cognition
Cognition
World model, causal, episodic, belief, counterfactual, self-model, hypothesis
Ledger — read. Control fabric
Valuation
Term structure, credit, surface, illiquid, cashflow, corporate actions
Ledger — read. Cognition — read
Intelligence
Lifecycle, training, meta-learning, adversary, simulation, risk policy
Ledger, control fabric
Optimisation
Solvers, routing gate, policy emitter
Ledger — read. IBM endpoints only
Control fabric
Pub/Sub shipping the payload down, outcomes up
Execution — publish only
Execution
Regional nodes. No external IP
Venues, fabric, Ledger — append
Ledger
Spanner
Nothing outbound
Wallet — read
Aggregation and adapters. Signing crate not linked
Venues, chains, custodians — read
Treasury — write
Corridors, transfer engine and gate, custody policy engine
Ledger, custodians, withdrawal APIs
Management
Cloud Build, Deploy, break-glass
All — audited
Ingestion reaches the outside world constantly and can reach nothing that moves money. That is the correct shape for the component with the widest external surface.
46.2 Controls
Layer
Control
Identity
Workload Identity Federation. Per-service identities, least privilege. No long-lived keys, enforced by organisation policy
Network
Global VPC, no external IPs, Cloud NAT with allowlist, Private Service Connect
Secrets
Market data, trading and withdrawal credentials are three separate sets, region-scoped, IP-restricted at the venue
Keys
Cloud KMS for signing, Cloud HSM for custody. Post-quantum algorithms for corridor signatures and long-lived secrets
Supply chain
One dependency graph. cargo-audit and cargo-deny. Signed images, attestation, Binary Authorization. No interpreter in the production image
Runtime
Distroless containers with a static binary, non-root, read-only root filesystem, no shell. The node runs bare under systemd
Plan and model integrity
Signed by the compiler and promoter, verified by the node before swap. A tampered plan is a trading control bypass
Poisoned information source
Sources scored continuously. A source whose facts repeatedly fail falsification is downweighted then dropped. No belief exceeds a threshold on a single source
Source discovery isolation
The dark web crawler runs in a hardened enclave with no path to any capital-moving component. It registers sources and never ingests content. The material-non-public-information exclusion is enforced at classification, before registration, and every crawl, rejection and registration is audited
Adversarial extraction input
Extracted facts carry confidence and provenance. Low-confidence extraction updates weakly and can never establish a causal edge alone
Wallet separation
The read path does not link the signing crate. Verified by dependency audit
Withdrawal defence in depth
Venue allowlist out of band, signed corridor, MPC policy share. Three independent points
Approval asymmetry
Halting is available everywhere with biometric or hardware key. Loosening, signing a destination, approving a family or market creation is portal-only with a hardware key
Kill switches
Two independent paths — Spanner flag polled and Pub/Sub broadcast. Either halts trading, quoting and transfers. Stale is treated as engaged
Audit
Every verdict including silence, every belief change, every corridor change, every transfer, every promotion, every optimisation run — immutable with retention lock
47. Observability
A live platform graph built from OpenTelemetry spans. Every hop emits a span with a stable node_id; the hot path writes to a bounded ring drained by a separate thread; graph-builder materialises nodes and edges into Spanner; the portal renders it as server-side SVG with click-through.
Category
Signals
Cognition
Causal edge count and confidence distribution. Belief calibration — when it says seventy percent, does it happen seventy percent. Episodic retrieval hit rate. Hypothesis throughput and refutation rate
Counterfactual
Veto profitability by rule, venue and regime. Feasibility rejection distribution. Unfunded family performance against funded
Self-model
Coverage gaps. Estimator error against declared bounds. Exploration spent and uncertainty resolved per unit
Valuation
Mark staleness by asset. Method distribution. Confidence-weighted portfolio value against naive sum
Ingestion
Source latency, reliability and disagreement. Entity resolution confidence. Extraction error rate
Strategy engine
Evaluation latency, strategies woken per event, firing rate per family, plan compile and swap
Netting
Netting ratio. Internal cross volume. Self-trade preventions
Per-strategy
Attributed P&L, live against holdout band, capacity use, crowding
Cycle economics
Detected, gated, fired, completed, resized, unwound per path. Expected against realised
Dispersion
Arrival spread per venue combination before and after equalisation. Per-venue p50 and p99
Market making
Quote rate against budget, message-to-trade ratio, spread capture net of adverse selection
Capital
Deployed share, reserve use, in-transit, unfunded commitments, liquidity ladder position
Solver
Classical against quantum by class. Routing decisions. Realised profit attributed to each solver&apos;s policy
Wallet
Balance staleness per adapter. Reconciliation deltas and breaks
Policy freshness
Age of all twelve shipped items per region
Belief calibration is the single most important metric. A platform whose seventy percents happen seventy percent of the time can size on confidence; one whose do not is worse than a platform with no confidence at all.
48. DevOps and Platform Engineering
  source -> Cloud Build -> test + cargo-audit + attest -> Artifact Registry
         -> OpenTofu apply (infrastructure)  |  Cloud Deploy / instance replacement
Stage
Gate
Build and test
Unit, integration, and replay against captured market data for every path
Static analysis
clippy, cargo-audit, cargo-deny. A new critical advisory blocks the build
Gate fixtures
Every risk-gate and transfer-gate rule, including all path extensions, has a passing and a vetoing fixture. Coverage of those crates must be total
Path and family simulation
Each exercised in sim against recorded data before enablement
Attestation
Signed provenance at build. Unsigned artifacts cannot deploy
Infrastructure
OpenTofu. Plan reviewed before apply. State in Cloud Storage with locking. Emits GCP and IBM resources only
Deploy — services
Cloud Deploy with gradual rollout and automatic rollback on error rate
Deploy — node
Blue-green instance replacement. Shadow mode before taking venue sessions
Admission
Binary Authorization. Only attested images run
Environment
Infrastructure
Purpose
dev
Cloud Run only. No node, no Spanner
Fast iteration. Costs almost nothing
sim
One node, replay harness, simulated venues with adaptive agents, shared Spanner
Full hot path against recorded and simulated data. Where correctness is proven
prod
Full stack, real venues, real capital
Live
Every node deployment runs in shadow first: it connects, ingests, evaluates and gates, but discards orders. Divergence from the running node beyond a threshold blocks promotion. Source control is hosted third-party as a documented exception, mirrored to Cloud Storage on every push.
49. Reliability and Failure Modes
Event
Behaviour
Recovery
Ingestion source unavailable
That source&apos;s facts age. Beliefs depending on it widen. Event-driven strategies reduce
Automatic on recovery
Sources disagree materially
Distribution widens rather than picking a side. Recorded as an episode
Resolves as evidence arrives
Causal re-estimation fails
Prior graph remains with a staleness marker. Regime conditioning reverts to unconditional
Next successful run
Episodic retrieval unavailable
Analogical sizing falls back to prior. Recognition-dependent strategies pause
Automatic
Belief stale past TTL
Fixed conservative multiplier. Region reports operating without belief
Fresh prior
Counterfactual scoring down
Learning slows. Zero trading impact
Restart
Valuation engine down
Unpriced assets freeze at last mark and are flagged. Priced assets unaffected
Restart
Mark past staleness limit
Position excluded from collateral and leverage until refreshed
Refresh
Capital call arrives unexpectedly
Reserve covers it. If insufficient, ladder unwinds from the top; shortfall is severity one
Human review
Plan fails to compile
Previous plan stays active. Alert
Fix specification
Intent buffer overflow
Excess vetoed, never allocated. Alerted
Investigate the family
One strategy misbehaves
Per-strategy limits contain it. Dropped from the net
Decay detector or manual
Quote rate budget exhausted
Threshold widens; low-value instruments stop repricing
Automatic
Toxic flow on a venue
Quotes widen or pull on that venue only
Automatic as signal decays
One leg&apos;s venue drops
Deadline forces resize or unwind. Venue quarantined
Automatic on recovery
Reference price stale
Mirrored trading disabled. Direction gating cannot be trusted
Fresh reference
Mirror drift past hard band
Depleted region restricted to reducing direction
Corridor transfer
Node crash
Reconciles against every venue including resting orders and quotes before resuming
Under 3 minutes
Central unavailable
Last shipment in force. Narrows in defined order to intra-venue only
Fresh policy
Quantum backend unavailable
Classical used. Recorded as a miss. No impact
None required
Ledger unreachable
Halt new intents. Manage open orders. Buffer fills to durable disk
Drain, reconcile
Reconciliation break
That venue-asset halts. Never auto-corrected
Human investigation
Transfer velocity breaker
All transfers halt. Trading continues until mirrors drift
Human reset
Region failure
That region dark. Mirrors suspend. Others unaffected
Redeploy, reconcile
Drawdown breaker
Mass cancel, flatten, halt the family or platform
Human re-enable only
49.1 Targets
Metric
Target
Node availability during market hours
99.9 percent
Strategy evaluation, p99
Under 90 µs
Wire to first order, p99 internal
Under 1.3 ms
Belief calibration error
Within tolerance across all belief classes
Netting ratio
Above 1.5
Cycle completion — Path 1 / Path 2
Above 97 / 93 percent
Arrival dispersion after equalisation
Under 1.5 ms p99
Quote message-to-trade ratio
Inside every venue&apos;s requirement with headroom
Mirror drift inside soft band
95 percent of the time
Live-versus-holdout consistency, funded strategies
Within band for 80 percent
Mark staleness
Zero positions past limit supporting leverage
Effective breadth
Above the allocator&apos;s floor
Reconciliation breaks
Zero unexplained. Any is severity one
Unauthorised transfer attempts reaching execution
Zero. Any is a security incident
50. Feasibility Assessment
50.1 Practical Now
Capability
Note
Strategy compilation with CSE and subscription indexing
A compiler problem with well-understood techniques
Intent netting with pro-rata attribution
Arithmetic and bookkeeping. High value, low difficulty
Incremental hierarchical risk aggregation
Atomic counters. O(1) by construction
Executable graph and indexed cycle detection
Standard graph algorithms with a precomputed index
Paths 1 and 3
Path 3 is the highest-value new capability and among the easiest — a band comparison
Latency-equalised dispatch
A timer wheel and a rolling percentile. Large effect, small effort
Counterfactual scoring
The data is already captured. Reconstruction and scoring is batch arithmetic
Episodic memory
Compression, storage and approximate nearest neighbour are all mature
Belief state with confidence-weighted sizing
Bayesian updating and a multiplier. The discipline is in calibration measurement
Streaming sufficient statistics
Welford, EW covariance, t-digest, reservoir. All constant-memory and straightforward
Deflated Sharpe and purged CV
Published methods. Implementing them correctly is the entire game
Corridor-bounded autonomous transfers
How institutional treasury automation already works
Term structure, corporate actions, volatility surface
Standard quantitative finance. Well-documented
Passkey identity and step-up
Mature web standards
50.2 Advanced But Achievable
Capability
What makes it hard
Ten thousand strategies inside the evaluation budget
Compiler, index, tiering and layout must all work together
Entity resolution at scale
The same company under a ticker, legal name, registry identifier and colloquial name. Getting it wrong corrupts everything downstream
Causal edge estimation with honest confidence
Observational causal discovery is unreliable. Natural experiments and own flow are the defensible sources
Belief calibration across classes
Requires enough resolved outcomes per class. Slow to accumulate, and unusable until it has
Market making net of adverse selection
Quoting is easy; not being picked off is the problem
Family clustering reflecting stress correlation
Calm-market correlation understates stress correlation
Illiquid valuation defensible enough to lock capital against
Method choice and input quality. Marks decay and must be refreshed
Prediction market edge over the market&apos;s implied
Requires the world model to produce information the market has not priced
Rust quantum path
Three to five weeks. Bounded because one circuit family is needed
Joint cycle selection and path assignment
Larger than selection alone. May need decomposition sooner
50.3 Experimental
Capability
Assessment
Hypothesis generation producing genuine novelty
Generating candidates is easy. Generating candidates that are not reparameterisations is what effective breadth measures
Regime-conditional allocation beating unconditional
Well-understood in principle. Depends on the regime classifier being calibrated
Market simulation calibrated to reality
Adaptive agents that predict actual fills. Uncalibrated, it is confident expensive error
Adversarial modelling
Instrumentation must exist from the start. Value appears only at scale
Market creation
Requires belief, causality and valuation all working. The most lucrative and most dangerous item
Path 8 payoff equivalence
Greeks gating, assignment risk and four-leg options depth
Verified quantum advantage on Algorik&apos;s workloads
IBM targets community-verified advantage by end of 2026. The routing gate produces exactly the evidence needed
50.4 Not Feasible, and Why That Is Fine
Claim
Why not
Ten thousand independent alpha sources
Effective breadth is low hundreds. The platform measures this rather than pretending
Evaluating every strategy on every event
Impossible in a microsecond budget. Compilation, indexing and tiering replace it
Atomic cross-region cycles
Speed of light. Paths 3–6 remove the need
Quantum on the execution path
Network round trip alone exceeds the budget by three orders of magnitude
Instrument-level causal discovery
Unidentifiable from available data. Driver level is estimable
Approving ten thousand strategies by judgement
A person who approves ten thousand has approved none. Humans approve families
A language model deciding a trade, path or transfer
Latency and non-determinism. Either alone is disqualifying
Economic sense at two hundred dollars
Infrastructure at five times capital monthly. A correctness harness, not an engine, and the frontend says so
Sub-millisecond execution on public cloud
Requires colocation. Out of scope until proven necessary
Physical delivery requiring storage
Physical infrastructure, not software. Financially settled equivalents are in scope
51. Implementation Roadmap
Reordered from version 8. The two changes that matter: counterfactual scoring moves very early because it is nearly free and improves everything after it, and prediction markets move far earlier because they are the one arena where small capital is an advantage.
Phase
Deliverable
Duration
Exit
0 — Foundation
GCP org, OpenTofu, global VPC, identity with passkeys, Cloud Build and Deploy, Cargo workspace, PQC keys
4 wks
Signed artifact deploys end to end with no manual step
1 — Market data
Two venues, one asset class. Connectivity with latency measurement, normalisation, book, features, statistics engine, event-anchored capture
5 wks
7 days stable streaming, statistics converged, no raw stream retained
2 — Research loop
Strategy spec and compiler, backtest runner linking production crates, data reference resolver, statistical validator with deflated Sharpe and purged CV, Rust classical solvers
10 wks
GATE: a family survives holdout with honest significance after cumulative trial correction
3 — One strategy live
Full hot path, feasibility gate, netting, risk aggregates, Spanner ledger with attribution, first C3 node, shadow mode
8 wks
GATE: 30 days live, performance inside the holdout band, no unexplained break
4 — Counterfactual scoring
Shadow execution and scoring of every veto, filter and rejection. Fill simulation calibrated against actuals
4 wks
Every declined path scored daily. Rule calibration driven by evidence
5 — Ingestion and world model
Source adapters, extraction, entity resolution, entity graph, resolution sources
10 wks
Entities resolved above confidence threshold. Events linked to instruments
6 — Prediction markets
Event resolution engine, base rates, calibrated probability estimation. First arena where small capital is an advantage
8 wks
Calibrated positions live. Brier score beating the market&apos;s implied
7 — Episodic and belief
Episode store and retrieval, belief engine, confidence-weighted sizing shipped to regions
8 wks
Belief calibration measured and within tolerance. Sizing responds to confidence
8 — Causal inference
Causal graph over drivers, natural experiments, own-flow interventions, regime conditioning
12 wks
Edges with evidence. Regime-conditional allocation live and beating unconditional
9 — Self-model and exploration
Capability estimation, coverage mapping, exploration budget as a capital line item
6 wks
Exploration directed by uncertainty. Value of information measured
10 — Multi-strategy
Compiled plan at scale, netting under real contention, promotion pipeline automated
8 wks
500+ strategies. Netting ratio above 1.5. Attribution verified exact
11 — Arbitrage and market making
Executable graph, paths 1 and 2, dispersion control, quote engine with adverse selection
10 wks
Path 2 completion above 93 percent. Quoting net positive after adverse selection
12 — Wallet and treasury
Wallet adapters, reconciliation, destination registry, corridors, transfer gate, custody, MPC
10 wks
Every holding reconciled. Autonomous rebalancing, zero unauthorised attempts
13 — Web and mobile
Portal, portfolio, wallet, strategy marketplace, explanation, installable PWA, push drill
8 wks
Every operational question answerable. Kill switch exercised from mobile
14 — Valuation plane
Term structure, credit, volatility surface, corporate actions. Bonds and options enter the graph
12 wks
Fixed income and options live. Equities safe through a corporate action
15 — Optimisation at cadence
Family clustering, quantum solver in production, regime conditioning, multi-horizon, routing gate
10 wks
Family allocation driving real capital. Solver delta measured
16 — Multi-region
Second and third region, mirrors, direction gating, paths 3 through 6
12 wks
Three regions. Cross-region capture live via mirrors
17 — Illiquid and private
Illiquid valuation, commitments, capital calls, liquidity ladder, private markets workflow
14 wks
Private positions held and marked with method and confidence
18 — Adversarial and simulation
Adversary modelling, market simulator with adaptive agents, execution RL against reactive counterparties
12 wks
Simulator calibrated against actual fills. Execution tactics learned in simulation
19 — Market creation
Origination where no market exists, gated by valuation, causal explanation and human approval
ongoing
Per instrument class, on evidence
51.1 The Gates
Gate
Question
If no
End of Phase 2
Does a family survive holdout with honest significance after cumulative trial correction?
Stop. Do not build execution infrastructure. This is the most important gate in the document
End of Phase 3
Does it survive contact with a live venue, inside its holdout band?
Stop live trading. The gap is the finding
End of Phase 6
Is the platform&apos;s calibrated probability better than the market&apos;s implied on prediction contracts?
The world model is not yet producing edge. Fix it before building on it
End of Phase 8
Does regime-conditional allocation beat unconditional out of sample?
The causal graph is decorative. Keep it for explanation, stop sizing on it
Phases 4, 5 and 7 have exit criteria but no gate: a shortfall narrows scope, it does not stop the build. Phase 3 is the live-money gate. There is no Phase 4 gate.
51.2 Why This Order
Choice
Reason
Counterfactual scoring at Phase 4
Nearly free, and every phase after it learns faster because of it. Delaying it wastes the learning signal of everything built in between
Ingestion and world model before arbitrage
Prediction markets are the one arena where small capital wins, and they need world knowledge rather than speed. Building the fast path first optimises a constraint that does not bind
Belief before causality
Confidence-weighted sizing delivers value immediately and independently. Causal inference is slower, harder, and needs beliefs to act through
Self-model after belief and counterfactuals
It is built from calibration data those two produce. Earlier it would have nothing to measure
Valuation before private markets
Marks must be trustworthy before capital is locked for years against them
Market creation last
It requires belief, causality and valuation all working. Attempted earlier it is how a platform becomes exit liquidity rather than the house
52. Cost Model
Line
Phase 0–4
Phase 5–13
Phase 19
Cloud Run services and jobs
$110
$340
$700
Ingestion — feeds, extraction, rendering, licences
$80
$320
$720
Cognition — causal estimation, episodic, counterfactual
$40
$180
$420
Valuation engines
—
$90
$220
Backtest, validation, simulation compute
$140
$320
$640
Training compute, spot GPU
$100
$220
$400
BigQuery, derived series only
$30
$90
$200
Cloud Storage and episodic memory
$25
$70
$170
Compute Engine C3 execution nodes
$260 (1)
$260 (1)
$700 (3)
Cloud Spanner
$90
$150
$320
Memorystore, Pub/Sub, networking
$60
$150
$380
Observability
$70
$180
$420
KMS, Secret Manager, Cloud HSM
$10
$120
$180
IBM Quantum access
$0 (open)
$0–300
$600–2,000
TOTAL (approximate)
$1,015
$2,490–2,790
$6,070–7,470
Totals are the sums of the line items above; the ingestion line rose in version 10 for deep-web rendering and licences, and the totals were recomputed with it. Version 9 roughly doubled the early-phase cost, and the reason is worth stating plainly: ingestion and cognition are compute and feed costs that begin before any of the capability they enable is earning. That is a real trade, and it should be made with the arithmetic in Section 18 in view.
Lever
Effect
Counterfactual scoring is batch and preemptible
The highest-value cognitive capability is also among the cheapest — it runs on spare capacity
Ingestion source selection
Feed costs vary enormously. Start with free and low-cost sources; add paid feeds only when a strategy demonstrably needs one
Causal re-estimation cadence
Adaptive rather than scheduled. Re-estimate on regime change and on hypothesis resolution, not on a clock
Episodic sampling density
Store densely at high-surprise moments and sparsely in calm. Most of the value is in the tails
Routing gate before quantum
Instances with a provably small classical gap never reach a QPU
Owning no market history
The largest structural saving. A conventional design carries terabytes indefinitely; this pays a fetch cost only when research runs
Retiring dead strategies
Frees evaluation budget, backtest compute, message rate and capital simultaneously
The honest reading of this table against a two hundred dollar account is that the platform is not economically sensible at that capital — roughly a thousand dollars a month against two hundred is infrastructure at five times capital. It is sensible as a build, and the exploration and correctness value is real, but Section 18 should be read alongside every number here.
53. End-to-End Walkthrough
A filing arrives, and eleven minutes later a position exists. Every plane participates.
#
What happens
Plane
Elapsed
1
A regulatory filing is published. Ingestion fetches it, deduplicates against two other sources carrying the same story
Ingestion
t = 0
2
Facts extracted: party, action, amount, effective date. Each carries a confidence and a provenance
Ingestion
+8 s
3
Parties resolved to entities. One is a supplier to three listed manufacturers already in the world model
Ingestion
+2 s
4
A WorldEvent is emitted, linked to four entities and to a prediction contract that resolves on the same action
Ingestion
+1 s
5
The causal graph is traversed. Two edges are active from this driver: one to a commodity curve, one to equity volatility in the sector
Cognition
+3 s
6
Episodic memory is queried. Eleven analogous situations are retrieved, weighted by regime and causal context
Cognition
+40 ms
7
Their outcome distribution is combined with base rates and the filing&apos;s own confidence. A belief forms: the prediction contract resolves yes with probability 0.71, confidence moderate
Cognition
+2 s
8
The contract currently trades at an implied 0.58. The gap is material and the belief&apos;s confidence supports acting
Cognition
+1 s
9
Valuation marks the exposed equities and the commodity leg. One equity has a corporate action pending; its mark carries a wider confidence band
Valuation
+4 s
10
The capital engine checks the mandate: this family is permitted, capital is available outside reserve and unfunded commitments
Capital
+1 s
11
The risk engine confirms the envelope. Causal-driver exposure is checked — the platform already holds one position sharing this driver, so the size is reduced
Risk
+1 s
12
The optimiser folds it into the next allocation run. Regime is currently event-dense, which favours this family
Optimisation
+45 s
13
A grant is issued and shipped with the belief prior, the episodic digest and the causal digest
Optimisation
+3 s
14
The Americas node receives the payload, verifies signatures, and swaps it in by pointer. Trading never pauses
Execution
+2 s
15
A strategy in the event-driven family wakes on the new belief. Its predicate matches
Execution
+40 µs
16
The feasibility gate checks: the contract&apos;s minimum size is $1, the fee floor is 1 percent, the intended size clears both
Execution
+3 µs
17
Sizing applies the confidence multiplier. Moderate confidence produces roughly sixty percent of the size a high-confidence belief would
Execution
+2 µs
18
The intent joins netting. No opposing intent exists, so it passes through as a net intent of one contributor
Execution
+4 µs
19
The risk gate reads eight aggregates including causal-driver exposure. It stays silent
Execution
+14 µs
20
Inventory reserved, order dispatched, filled
Execution
+2 ms
21
The fill appends to the ledger with a contributor vector reaching back through the belief, the causal edge and the world event to the filing
Ledger
+30 ms
22
An episode is stored: what the world looked like, what was believed, what was done, what was declined
Cognition
+1 s
23
The paths declined are shadow-executed and scored: the larger size that confidence did not support, the equity leg the driver limit reduced
Cognition
batch
24
The self-model updates. This driver now has one more observation; coverage improves; the uncertainty that made confidence moderate narrows slightly
Cognition
batch
25
On resolution, the outcome joins the episode. Belief calibration updates. If the platform said 0.71 and this class of belief resolves at 0.71, calibration holds
Cognition
on resolution
26
The user opens the portal and asks why. The explanation traverses the chain in plain language: this filing, this supplier relationship, these eleven analogues, this confidence, this size
Experience
on demand
Steps 15 through 20 are the same microsecond hot path version 8 had. Everything before them is what version 8 could not do at all, and step 26 is what it could not explain.
54. Closed Decisions
Every design decision, with reasoning and a revisit trigger or a structural marker. Nothing is deferred to a prior version.
54.1 Architecture and Scale
Decision
Closed as
Reasoning
Revisit
Predicate language
Restricted and total. No loops, recursion, or calls into arbitrary code
A Turing-complete language makes evaluation cost unbounded, destroying the budget guarantee. Expressiveness comes through features
Structural
Hot-tier cap
1,200 per node
The count at which measured p99 evaluation reaches seventy percent of the 90 µs budget
Re-derive on node class or feature set change
Family count
128, working range 96 to 160
One binary per family plus encoding fits Heron directly. Granular enough at ~80 strategies per family
Re-cluster when correlation structure shifts
Internal crossing cap
Forty percent of gross intent per instrument
Above that a persistent internal market drifts from reality; below twenty percent value is left unclaimed
None
Trial budget
500 per family per quarter, corrected against cumulative lifetime trials
Per-batch correction is laundered by splitting a sweep
Structural
Causal graph scope
Drivers and mechanisms only, not instrument level
Instrument-level is unidentifiable and produces confident nonsense
Structural
Causal edge authority
Constrains sizing, enables explanation, never generates a trade
A wrong edge costs a suboptimal allocation, never an unbounded position
Structural
Belief formation location
Central only. Regions receive priors with TTL
A region has no world model. A local belief would be a guess with confidence attached
Structural
Counterfactual authority
Recalibration through the full approval path, never automatic
A rule that looks expensive may be the only thing before a tail event
Structural
Hypothesis admission
Requires a mechanism and a falsifier
Untestable claims cannot size positions
Structural
Language model role
Proposes, extracts, drafts. Never decides, approves, sizes
Latency and non-determinism disqualify it independently
Structural
54.2 Data and Memory
Decision
Closed as
Reasoning
Revisit
Snapshot window
90 days rolling
At 50,000 orders a day, ~4.5 million events. Ample for every execution model. ~9 GB
If a model class demonstrably needs longer
Bar fallback
Adopted. One-minute OHLCV, three years, active instruments only
The only hedge against a source withdrawing its archive. Near-zero cost
None
Source concentration
Two viable sources required before a universe is approved
Makes concentration risk a gate rather than a report
None
Episode sampling
Dense at high surprise, every action, regime transitions. Sparse in calm
Most information is in the tails
Adjust on retrieval hit rate
Episodic retention
Indefinite
Compressed meaning at single-digit GB per year is the cheapest thing in the document
None
Ingestion retention
Pass-through. Facts and links kept, source text discarded, manifest with hash kept
Identical discipline to market data. A filing is hundreds of KB; its facts are hundreds of bytes
None
Reconciliation tolerance
A formula per asset class: largest legitimate unmodelled accrual over one interval plus dust
A round number either halts on funding accrual or masks real breaks
On fee or funding model change
Ledger retention floor
Seven years
The common regulatory floor. The specific obligation is a compliance determination
On a compliance determination
Belief source threshold
No belief exceeds a confidence threshold on a single source
The obvious attack on a system that reads the world is to feed it something false
Structural
54.3 Valuation and Capital
Decision
Closed as
Reasoning
Revisit
Valuation confidence
Every mark carries method and confidence; confidence enters sizing and the envelope
A mark without a method is an assertion; an uncertain mark must not support leverage
Structural
Mark staleness
Past its limit, excluded from collateral and leverage
A stale mark supporting leverage is how illiquid books fail
None
Unfunded commitments
A hard reservation, never available capital
Failing a call typically forfeits the position
Structural
Private markets
A second operating mode sharing ledger, wallet, identity, capital engine and frontend
Building it as an asset class corrupts the fast path&apos;s assumptions
Structural
Feasibility gate position
Ahead of the profitability filter, in the hot path
The cheaper question; rejects most candidates at small capital
None
Exploration budget
A stated share of deployed capital in the mandate, accounted separately
Mixed into performance it reads as drag and gets cut
Adjust on measured value of information
Settlement bridge for equities
Venue margin only at first. No external credit line
A credit line is a commercial negotiation with covenant risk before equities have proven they contribute
If margin capacity becomes binding
Tax treatment
Pluggable per jurisdiction, configured not hard-coded. Lot tracking architectural
Rules differ by domicile and change
On jurisdiction change
Market creation gating
Valuation, causal explanation of participant absence, bounded exposure, per-class human approval
Origination without understanding why nobody else quotes is how a platform becomes exit liquidity
Structural
54.4 Quantum, Interfaces and Deployment
Decision
Closed as
Reasoning
Revisit
Solver approach
QAOA for all QUBO workloads, three layers, SPSA outer loop
One circuit family means one implementation. VQE adds ansatz complexity for no demonstrated gain
Per-class divergence after the tracker has data
ISA compilation
IBM Transpiler Service, routing pattern cached per problem size and backend
Removes the hardest work and keeps the dependency inside IBM
Rust ISA emission if transpiler round trips exceed fifteen percent of solver time
IBM plan tier
Open through Phase 15
Formulation runs in batches. Reserved capacity before the routing gate proves need is spending on schedule
Flex when the gate shows a material gap on more than thirty percent of instances
Mobile
Installable Leptos PWA. No native shell
Web Push and WebAuthn cover the two capabilities that justified native. One hundred percent Rust from one codebase
Native shell only if the push drill misses target
Session wallet connection
Not adopted
A third-party relay to save a registration step corridors already require. Algorik is the fund
None
Frontend graph interactivity
Server-side SVG with click-through
A JavaScript dependency in a platform whose supply chain argument rests on one dependency graph
None
First asset class
Crypto spot, three venues, one region
Continuous trading, instant settlement, API access, genuine cycles. Every other class adds a dependency before anything is proven
None
First cognitive arena
Prediction markets, Phase 6
The one arena where small capital is an advantage. Needs world knowledge, not speed
None
First mirror pair
BTC between Americas and Europe
Deepest liquidity in both, fungible, smallest basis risk
None
Colocation
Out of scope
Every dollar on latency before selection and placement work is spent on the wrong constraint
More than twenty percent of profitable cycles lost to speed
Source control
Third-party host, mirrored to Cloud Storage on every push
Neither vendor has a credible Git host. The mirror makes the exception reversible in a day
If a vendor ships one
OpenTofu
Retained
Declarative configuration, no runtime presence, no Rust alternative
None
Dark web discovery
Registration and defensive monitoring only. Hard exclusion on material non-public information, enforced at classification
Trading on non-public information is insider trading regardless of source. The line is legal, not technical, and is built so crossing it is not possible through the layer
Structural
Discovery pulls sources, not content
The crawler registers feeds and never hoards. Pass-through applies
Data gravity, and the legal risk of holding what should not be held, are both avoided by never copying
Structural
Deep web is a first-class source tier
Crawled broadly, with rendering, form-driving, registration and licence management. Feeds the full pipeline including training
The deep web is unindexed, not illicit; it is where lawful, underused information lives and where small capital has an informational edge
None
Deep-web access class is a gate
Every source carries an access class (open query, API, registered, licensed, rendered, bulk) and a terms-of-use status. Restricted sources are not registered. Circumventing an access control, violating terms, over-automating credentials, or collecting personal data on private individuals is excluded at classification
A lawful breadth advantage becomes a liability the moment access is not clean. The line is kept at the front of the pipeline, not filtered later
Structural
Deep web trains; dark web defends
Deep-web facts enter world model, causal graph, beliefs, episodes and training. Dark-web crawling registers threat indicators only and never feeds cognition or training. The two are never merged
The distinction is legal, not technical: deep-web content is unread, dark-web content is unlawful to trade on
Structural
Asset class registry as a gate
An unregistered class cannot be traded
Turns hundreds of classes from a capability claim into an enumerable, governed set
None
Divestment is first-class
Position lifecycle engine with explicit orphan, unwind and expiry handling
A position orphaned or unwound arbitrarily is how a good book becomes a loss on the way out
Structural
DeFi venues carry their own model
Pool-math slippage, block-time execution, MEV and contract risk. Observe-only until present
Treating a decentralised exchange as an order book is a category error with real capital at risk
Structural
55. Empirical Questions the Roadmap Answers
Not design decisions. Questions no amount of design can settle, each with a phase that answers it and a defined consequence if the answer is unfavourable.
Question
Answered by
If no
Does a durable, cost-adjusted edge exist at all?
Phase 2 — a family surviving holdout after cumulative trial correction
Stop. Do not build execution infrastructure
Does it survive contact with a live venue?
Phase 3 — 30 days live inside the holdout band
Stop live trading. The gap is the finding
Are the constraints protective or merely expensive?
Phase 4 — counterfactual scoring of every veto
Recalibrate with evidence. A finding either way
Can the platform beat a prediction market&apos;s implied probability?
Phase 6 — Brier score against market-implied
The world model is not producing edge. Fix it first
Is confidence calibrated?
Phase 7 — does seventy percent happen seventy percent
Stop sizing on confidence. Uncalibrated is worse than none
Does causal reasoning beat correlation out of sample?
Phase 8 — regime-conditional against unconditional
Keep the graph for explanation. Stop sizing on it
Does directed exploration beat undirected?
Phase 9 — uncertainty resolved per unit spent
Revert to flat allocation
Does netting pay off?
Phase 10 — netting ratio and effective breadth
The set lacks diversity. More variants make it worse
Is market making net profitable after adverse selection?
Phase 11
Disable the family
Do mirrors capture more than doubled capital costs?
Phase 16
Unwind. Revert to hedged and passive mechanisms
Are illiquid marks defensible enough to lock capital against?
Phase 17 — realised exits against prior marks
Do not hold private positions
Does the simulator predict actual fills?
Phase 18
Do not learn execution from it
Does quantum beat the classical baseline on real instances?
Continuously — the advantage tracker
Classical remains production. The instrumentation was worth building
Is the platform economically sensible at the user&apos;s capital?
Continuously — cost against attributed return
Say so plainly in the frontend
56. Implementation Rules
Written to be handed directly to an implementer or a coding agent as a standing constraint. All seventy-eight, in full.
56.1 Language, Money and Memory
#
Rule
1
All application code is Rust. If a task appears to require another language, stop and raise it.
2
All managed services are Google Cloud or IBM. Counterparties and information sources are not managed services.
3
Money is rust_decimal. Floating point for any monetary quantity is a review rejection.
4
No heap allocation inside the hot-path decision loop. Arena-allocate at start, reset per cycle.
5
No unbounded collection in hot-path code. Fixed capacity and an explicit eviction rule.
6
No unsafe outside a small set of reviewed primitives, each with a documented invariant.
7
Shared domain types live in the types crate. Never redefine a domain type inside a service.
56.2 Strategies, Netting and Risk
#
Rule
8
Strategies are declarative specifications compiled into a shared plan. Never arbitrary code executed per event.
9
The predicate language is total. No loops, recursion, or calls into arbitrary code.
10
No strategy sends an order. Strategies produce intents; intents are netted, gated, then executed.
11
Risk reads aggregates, never strategy lists. Any check that iterates strategies is a defect.
12
Every fill carries its contributor vector. Per-strategy P&L is exact, never estimated.
13
Arbitrage cycle legs carry a no-net flag and pass through netting untouched.
14
Quotes reserve their backing inventory. A quote with unreserved inventory is a defect.
15
A compiled plan exceeding the evaluation budget fails to compile. It is never deployed.
16
Compiled plans and models are signed at source and verified by the node before swap.
17
Both gates return veto or silence. Neither ever returns approval. Errors and timeouts are vetoes.
18
Cycles are gated as a unit. Never approve or fire a leg independently.
19
Path 3 requires a direction check against the inventory band and a reference price inside TTL. Both, every time.
20
Path 4 requires hedge availability verified before the first leg fires.
21
Reservation is settlement-aware. Check availability at the time the leg needs it.
22
Dispatch is latency-equalised wherever more than one venue is involved.
23
Feasibility is checked before profitability. Minimum size, tick, fee floor, gas, depth.
56.3 Statistics and Promotion
#
Rule
24
Promotion requires multiple-testing correction. Ranking by raw backtest Sharpe is forbidden.
25
Deflated Sharpe is corrected against cumulative lifetime trials per family, never per batch.
26
Humans approve families and thresholds. Statistics approve strategies.
27
The backtest runner links the production crates, including belief and feasibility. It never reimplements the compiler, executor, netting or gate.
28
Every gate rule, including all path extensions, has a passing and a vetoing fixture. Coverage of riskgate and transfer-gate must be total.
29
Every strategy family or arbitrage path is exercised in sim against recorded data before enablement.
30
Every streaming estimator declares its error bound and is monitored against it. An estimator without a bound is a guess.
31
A universe with fewer than two viable registered data sources cannot be promoted past validation.
56.4 Data
#
Rule
32
The raw stream is never persisted. It updates statistics and event-anchored snapshots, then is discarded.
33
Every retained byte belongs to a declared retention class. Data with no class does not get written.
34
External history is referenced by manifest with a content hash, never copied into permanent storage.
35
Research caches are TTL-scoped and reaped. A cache outliving its TTL is a defect.
36
Ingested source text is not retained. Facts, entity links and a manifest with a hash are.
37
Extracted facts carry provenance and confidence. A low-confidence extraction can never establish a causal edge alone.
38
The discovery crawler registers sources and never ingests content. It runs in an isolated enclave with no path to any capital-moving component.
39
No material non-public information is ever registered or acted on, regardless of where it was found. Enforced at classification, before registration.
40
Every discovered source carries a terms-of-use status. Restricted sources are not registered. Licensed sources are used within licence with per-source credentials. Politeness is enforced.
41
Every deep-web source carries an access class. Access never circumvents a paywall or access control, never shares or over-automates credentials beyond a source&apos;s terms, never uses credentials that are not the platform&apos;s own, and never collects personal data on private individuals. A source requiring any of these is rejected at classification.
42
Source freshness is measured, not assumed. A source is ranked on how far its facts preceded the same fact appearing elsewhere.
43
Deep-web content feeds the full pipeline including training. Dark-web content feeds defensive monitoring only. The two are never merged.
44
An asset class with no valuation engine, settlement convention or eligible family cannot be registered, and an unregistered class cannot be traded.
45
A venue is verified against measurement before production, never enabled from its own documentation alone. DeFi venues are observe-only until their pool-math, MEV and contract-risk models exist.
46
A position is never orphaned silently. On strategy retirement it is reassigned or scheduled for unwinding. A position with no owner is a reconciliation break.
56.5 Cognition
#
Rule
47
Every belief carries a distribution, evidence, a causal path and a TTL. A point estimate presented as a belief is a defect.
48
Position size is a function of confidence. A strategy that ignores the confidence multiplier does not pass review.
49
Absence of evidence and conflict of evidence are represented distinctly and produce different behaviour.
50
Every causal edge carries evidence, confidence and the conditions under which it holds. An edge without conditions is not an edge.
51
Causal edges constrain sizing and produce explanation. They never generate a trade directly.
52
Every veto, filter and feasibility rejection is shadow-executed and scored. A declined path not scored is a discarded signal.
53
A control may only be loosened through the full approval path, never automatically from counterfactual evidence.
54
Every hypothesis states a mechanism and a falsifier before it may be tested.
55
Exploration capital is a declared line item, accounted separately from return-seeking capital.
56
No belief exceeds a confidence threshold on a single information source.
57
Belief calibration is measured continuously. Sizing on uncalibrated confidence is a defect.
58
Every fill is attributable through belief and causal edge back to the observation that produced it.
59
Language models propose, extract and draft. They never decide, approve, size or route.
56.6 Valuation, Capital and Money
#
Rule
60
Every valuation carries a method and a confidence. A mark without a method is an assertion.
61
A mark past its staleness limit is excluded from collateral and from leverage calculations.
62
Unfunded commitments reserve capital. They are never counted as available.
63
Tax lots are tracked per position. Lot selection is part of the exit decision, not a reporting step.
64
Market creation requires a defensible valuation, a causal explanation for participant absence, bounded exposure, and per-class human approval.
65
The wallet read path does not link the signing crate. Verified by dependency audit, not convention.
66
The wallet never writes a correction to the ledger. A reconciliation break halts and alerts.
67
A destination is inert until a human signs it into a corridor with a hardware key and the delay elapses. Registration is not authorisation.
68
Reconciliation tolerance is derived from legitimate unmodelled accrual, never a round number. A persistent delta inside tolerance opens a defect ticket.
56.7 Quantum, Interfaces and Platform
#
Rule
69
IBM Quantum is reached over the Runtime REST API with OpenQASM 3 payloads. No Python in the platform.
70
Classical solvers and the comparison harness are built before the quantum path. Quantum must never block the Optimisation Plane.
71
Mobile is the Leptos PWA. A native shell is built only if the push reliability drill misses target, and it would contain view code only.
72
Halting is available on every surface. Loosening a limit, signing a destination, approving a family or market creation is portal-only with a hardware key.
73
There is no manual trading path on any surface. Every order originates from a strategy or a cycle.
74
Every service emits OpenTelemetry spans with a stable node_id. A component that cannot be traced does not ship.
75
Containers are distroless with a static binary, non-root, read-only root filesystem, no shell. The node runs bare under systemd.
76
cargo-audit and cargo-deny run in the pipeline. A new critical advisory blocks the build.
77
Infrastructure is OpenTofu and emits GCP and IBM resources only.
78
Every closed decision carries a revisit trigger or is marked structural. Reopening one without its trigger firing requires the same scrutiny as making it.
57. Summary
Question
Answer
What is Algorik?
A cognitive investment platform. It observes the world through information rather than prices alone, models what causes what, remembers what it has seen, holds beliefs with stated confidence, values assets that have no price, allocates with quantum optimisation, acts in microseconds through regional nodes, and learns from every path it declined.
What made it cognitive?
Four capabilities: model the world causally, remember episodically, reason counterfactually, and know what it does not know. Everything else is elaboration on those four.
How does it run tens of thousands of strategies?
They are compiled, not iterated. One shared evaluation plan with deduplicated features, an inverted subscription index, tiering so most never touch the hot path, and belief and feasibility bound in at compile time. Seventy microseconds for the engine.
What stops them flooding the venues?
Intent netting. Every intent, quote and cycle leg converges on one stage, nets per instrument, and leaves as one order. Disagreeing strategies cross internally and never reach a venue.
How does risk keep up?
Incremental aggregates including causal-driver exposure. Ten atomic adds per fill. A check costs the same at ten strategies or ten thousand.
How is capital allocated?
Cluster into 128 families, allocate across them on quantum hardware conditioned on regime and horizon, distribute within classically. Only the user, the capital engine and the optimiser inside the envelope can say yes.
What opened up?
Roughly sixty percent of investable wealth that has no continuous price, plus prediction markets and event-driven strategies — the arenas where small capital is an advantage.
What is cheapest and most valuable?
Counterfactual scoring. The gate already logs every veto; the data is already captured; nothing scored them.
How does money move?
Autonomously inside corridors. Adding a destination takes 24 hours and a hardware key. The Wallet sees every holding and can move none of it.
Who decides?
Humans approve families, thresholds, destinations and market creation. Statistics approve strategies. The optimiser allocates. A deterministic gate vetoes. No model holds authority anywhere.
What data does it keep?
Almost none. Statistics, episodes, its own irreplaceable records, and manifests pointing at everything else. Nothing grows with tick rate.
How does it find new data?
A discovery crawler over the surface, deep and dark web registers sources and creates feeds — it never hoards content. The deep web is the real breadth gain: lawful, unread information from regulatory portals, dockets, registries, procurement and industry databases, feeding cognition and training. Dark web monitoring is defensive only, with a hard exclusion on non-public information.
Can it trade hundreds of classes and divest cleanly?
Yes, once the asset class registry, venue onboarding, settlement calendars, hedge map and cross-margin model are populated, and the position lifecycle engine makes divestment first-class rather than a side effect.
What is the biggest risk?
That ten thousand backtested strategies contain five hundred that look significant by chance. Cognitive machinery does nothing about that, which is why the Phase 2 gate is still the most important line in the document.
What would make this fail?
Sizing on the cognitive planes before calibration is proven. An uncalibrated confidence is worse than no confidence, because it sizes up precisely when it should size down.
ALGORIK  ·  Master Architecture & Application Blueprint  ·  Version 10.1  ·  Complete
Observe. Understand. Believe. Value. Allocate. Act. Remember.