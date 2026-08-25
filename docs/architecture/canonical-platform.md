# Canonical platform — the diagram as a manifest

Transcribed from the canonical architecture diagram ("World's Smartest
Multi-Regional AI + Quant Trading Platform"). This file is the machine-readable
source of truth that `diagram-reconciliation.md` scores the repository against.
Component ids are stable; do not renumber them.

## Layer 1 — Autonomous Data Mesh (`L1`)
Find and ingest everything important.

| id | Component |
|---|---|
| L1.1 | Market Data — all asset classes |
| L1.2 | Order Books — Level 1, 2, 3 |
| L1.3 | News & Social — events |
| L1.4 | Company Data — filings, earnings |
| L1.5 | Macro & Econ — indicators |
| L1.6 | On-Chain / DeFi — NFTs, wallets |
| L1.7 | Alt Data — satellite, IoT, mobility, web |
| L1.8 | Reference Data — prices, FX, rates, ids, corp actions |
| L1.9 | AI Feed Discovery Agents |
| L1.10 | Continuous Collectors (Rust) |
| L1.11 | Deduplication, normalization, time sync, enrichment |
| L1.12 | Source scoring & quality |
| L1.13 | Event fingerprinting |

Data principles: always-on discovery; real-time quality scoring; hot/important/
irrelevant classified in milliseconds; store intelligence not noise; auto-adapt
to new sources.

## Backbone (`BB`)
| id | Component |
|---|---|
| BB.1 | Global streaming backbone (diagram says Google Pub/Sub; ADR 0011 replaced this with the in-tree `qip-transport` mesh — see reconciliation) |

## Layer 2 — Regional AI Brains (`L2`)
Seven regions: US East, US West, Europe, Asia, APAC, S. America, Middle East.
Each region carries the identical capability set.

| id | Component (per region) |
|---|---|
| L2.1 | Market understanding |
| L2.2 | Anomaly detection |
| L2.3 | Liquidity & impact |
| L2.4 | Local alpha & arbitrage |
| L2.5 | Cash & inventory |
| L2.6 | Risk & limits |
| L2.7 | Ultra-fast decisioning |
| L2.8 | Rust engine + local models |
| L2.9 | Publish regional state deltas: opportunities, confidence, liquidity, capital, risk |

## Layer 3 — Global Opportunity Brain (`L3`)
| id | Component |
|---|---|
| L3.1 | Global knowledge graph — assets, currencies, derivatives, companies, commodities, venues, cash, collateral, regions |
| L3.2 | Cross-market correlation |
| L3.3 | Multi-leg arbitrage engine (2-leg, 3-arm, N-leg) |
| L3.4 | Event impact & catalyst detection |
| L3.5 | Structural mispricing detection |
| L3.6 | Strategy discovery (statistical + AI) |
| L3.7 | Liquidity topology understanding |
| L3.8 | Regime & cycle detection |
| L3.9 | Outputs: ranked opportunities, required legs, expected edge, time decay, execution complexity, required capital |

## Layer 4 — Capital Brain (`L4`)
| id | Component |
|---|---|
| L4.1 | Opportunity scoring |
| L4.2 | Expected alpha calculation |
| L4.3 | Position sizing & capital allocation |
| L4.4 | Cash, collateral & margin management |
| L4.5 | FX & multi-currency exposure management |
| L4.6 | Risk engine — real-time VaR / CVaR |
| L4.7 | Hedge & inventory optimization |
| L4.8 | Trade / no-trade decision |
| L4.9 | Identity: return − costs − risk − slippage − latency decay − capacity − correlation − constraints = expected usable alpha |
| L4.10 | Actions emitted: trade/no-trade, size, region(s), venue(s), strategy, hedge, cash to reserve, expiration/TTL |

## Layer 5 — Regional Execution Mesh (`L5`)
Seven execution engines, same regions as L2.

| id | Component |
|---|---|
| L5.1 | Smart order routing |
| L5.2 | Venue selection |
| L5.3 | Smart order slicing |
| L5.4 | Fill optimization |
| L5.5 | Hedging |
| L5.6 | Inventory |
| L5.7 | Failover |

Execution principles: local decisioning; microsecond execution; pre-trade risk
checks; partial fill handling; dynamic repricing; resilient & redundant.

## Layer 6 — Outcomes Capture (`L6`)
| id | Component |
|---|---|
| L6.1 | Fills & P&L |
| L6.2 | Slippage & costs |
| L6.3 | Partial fills |
| L6.4 | Missed opportunities |
| L6.5 | Market impact |
| L6.6 | Strategy attribution |
| L6.7 | Exposures over time |
| L6.8 | Risk & limit utilization |

## Layer 7 — Evolution Brain (`L7`)
| id | Component |
|---|---|
| L7.1 | Counterfactual digital twin |
| L7.2 | Model training (Vertex AI) |
| L7.3 | Strategy evolution engine |
| L7.4 | Model validation & backtesting |
| L7.5 | Policy distillation (small models) |
| L7.6 | Deploy to all regions |
| L7.7 | IBM Quantum optimization, offline/nearline: portfolio optimization, N-leg topology search, capital allocation, scenario search |

## Cross-cutting intelligence layers (`X`)
| id | Component |
|---|---|
| X.A | Counterfactual digital twin — simulate every decision and every alternative |
| X.B | Contextual model router — right model/agent/strategy for the context; max accuracy, min compute |
| X.C | Predictive capital fabric — predict where opportunities emerge, pre-position cash/collateral/inventory |
| X.D | Confidential global intelligence — confidential computing, share insights without raw data |
| X.E | Quantum-centric learning fabric — IBM Quantum + HPC + AI for hard problems |

## Governance & guardrails (`G`)
| id | Component |
|---|---|
| G.1 | Global risk policies |
| G.2 | Regulatory controls |
| G.3 | Audit trail (immutable) |
| G.4 | Kill switch |
| G.5 | Model governance |
| G.6 | Data lineage |
| G.7 | Compliance rules |
| G.8 | Stress testing |

## Non-functional requirements (`N`)
| id | Requirement |
|---|---|
| N.1 | Ultra-low latency |
| N.2 | Massive scalability |
| N.3 | High availability |
| N.4 | Disaster recovery |
| N.5 | Multi-region active |
| N.6 | Observability |
| N.7 | Cost efficiency |

## Technology stack named by the diagram (`T`)
| id | Technology | Note |
|---|---|---|
| T.1 | Google Cloud (multi-region) | in use |
| T.2 | Pub/Sub | superseded by in-tree mesh, ADR 0011 |
| T.3 | Spanner | flag exists, adapter does not |
| T.4 | Vertex AI | module exists, flag off |
| T.5 | BigQuery | adapter exists, needs egress proxy |
| T.6 | Dataflow | not provisioned |
| T.7 | GKE | in use |
| T.8 | Cloud Storage | adapter exists, needs egress proxy |
| T.9 | Security Center | project-scoped only; needs org activation |
| T.10 | Confidential VMs | flag exists, off |

## Flows the diagram draws
| id | Flow |
|---|---|
| F.1 | L1 → BB (normalized events onto the backbone) |
| F.2 | BB → L2 (each region consumes) |
| F.3 | L2 → L3 (regional state deltas up) |
| F.4 | L3 → L4 (ranked opportunities into capital allocation) |
| F.5 | L4 → L5 (actions down to regional execution) |
| F.6 | L5 → L6 (execution outcomes captured) |
| F.7 | L6 → L7 (outcomes feed learning) |
| F.8 | L7 → L2 (distilled policy deployed to all regions) |
| F.9 | X.* ↔ all layers (cross-cutting) |
| F.10 | G.* ↔ all layers (governance overlay) |

## Standing deviation from the diagram
This platform is **paper-trading only**. L5 must route exclusively to the paper
execution engine or a provider sandbox; no live-order submission path may exist.
Every "execute" in the diagram is to be read as "execute against the simulator".
