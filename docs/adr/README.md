# Architecture decisions

Each record states what was decided, what it costs, and what would make it
wrong. A decision with no stated cost has not been thought about, and one with
no stated reversal condition cannot be revisited honestly.

| # | Decision |
|---|---|
| [0001](0001-rust-everywhere.md) | Rust for everything, including the web interface |
| [0002](0002-two-dependencies.md) | Two third-party dependencies |
| [0003](0003-paper-trading-by-default.md) | Paper trading by default, with a deployment ceiling |
| [0004](0004-capability-gated-agents.md) | Agents reach facilities only through capability gates |
| [0005](0005-confidence-is-arithmetic.md) | Confidence is computed, never assigned |
| [0006](0006-classical-baseline-always.md) | A classical baseline for every quantum result |
| [0007](0007-exact-attribution.md) | Attribution must reconcile exactly |
| [0008](0008-edge-cells-decide-alone.md) | Edge cells decide alone, on capital granted in advance |
| [0009](0009-tiered-dependency-policy.md) | A tiered dependency policy, so the core stays at two |
| [0010](0010-what-gets-deployed.md) | Four of the six application crates are deployed, and the other two are not |
| [0011](0011-everything-in-rust-on-kubernetes.md) | Everything is Rust on Kubernetes; IBM Quantum is the only integration — *Kubernetes half superseded by 0024* |
| [0012](0012-where-a-library-earns-its-place.md) | A dependency is admitted only where getting it wrong is silent, the problem is specialist, and a maintained implementation exists |
| [0013](0013-identity-verification-earns-a-dependency.md) | Verifying an identity token earns a dependency; issuing sessions does not |
| [0014](0014-one-design-system-four-surfaces.md) | One design system, four surfaces, no second router |
| [0015](0015-licensed-templates-are-the-visual-source-of-truth.md) | The licensed templates are the visual source of truth |
| [0016](0016-repository-layout.md) | One layout: backend, frontend, data, infrastructure — each top-level directory answers one question |
| [0017](0017-gitops-delivery.md) | Delivery becomes GitOps: a Helm chart, Argo CD sync, Kargo promotion — *superseded by 0024* |
| [0018](0018-the-console-reaches-the-platform-over-the-vpc.md) | The console reaches the platform over the VPC, on an internal load balancer, as viewer |
| [0019](0019-identity-platform-is-the-only-identity-store.md) | Identity Platform is the only identity store, and the session is a sealed cookie |
| [0020](0020-two-runtime-topologies-and-the-order-to-resolve-them.md) | Two runtime topologies exist; neither is deleted, and the order to resolve them is fixed — *direction decided by 0022; the code executed under 0024, the apply still unauthorised* |
| [0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md) | The blueprint expects live capital; the paper-trading boundary wins and the structure is built without it |
| [0022](0022-the-algorik-blueprint-is-the-architecture-of-record.md) | The Algorik blueprint is the architecture of record; Kubernetes and Next.js become transitional, and no live-capital path is authorised by that |
| [0023](0023-real-trading-is-the-destination-and-the-opening-is-gated.md) | Real trading is the destination and paper trading the harness; the opening is a ten-step gated sequence that this record does not authorise |
| [0024](0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md) | The blueprint runtime is provisioned in code and the GitOps runtime is retired; nothing was applied, and the apply still needs a plan a person reads |
| [0025](0025-the-rust-frontend-boundary-and-the-leptos-dependency.md) | *Proposed* — the Rust first-party boundary is `qip-web`; Leptos is a reversal of 0001 and 0012, admissible only against 0001's own reversal condition |
| [0026](0026-telemetry-export-bounded-opentelemetry-versus-prometheus-exposition.md) | *Proposed* — keep the Prometheus exposition for metrics and let the collector translate; spans through the blueprint's own bounded ring with `serde_json`; an OpenTelemetry crate rejected |
| [0027](0027-concentration-limits-are-a-share-of-what.md) | A concentration cap is a share of **equity**, not of gross — option (a) taken and applied at `eca7ebb`: `LimitKind::MaxAxisWeight` replaces the two share-of-gross entries in `LimitSet::conservative_default` on the same axes at 0.35 and 0.60, so the first order into a fed book is admitted and a sector past the cap is still refused; `MaxConcentration` stays in the type, out of the default set, for §28.1's family cap |
| [0028](0028-openobserve-is-adopted-as-a-deliberate-deviation-from-section-2-1.md) | OpenObserve is adopted as the observability backend over OTLP, on ephemeral storage — a named, deliberate deviation from §2.1's Google Cloud/IBM-only managed-services rule, superseding ADR 0026's recommendation by explicit owner instruction |
| [0029](0029-the-normaliser-is-removed-rather-than-recorded-as-research-only.md) | `qip-normalization` is deleted rather than recorded research-only: nothing constructed it, its unmapped-symbol guard could not fire, and four documents cited it as a control — the gap it leaves is named here rather than disguised; applied: the crate, its workspace and dev-dependency lines and its benchmark are gone, the truth-loop suite owns its stage-4 identity table, and the workspace is 58 crates |
| [0030](0030-openobserve-is-reachable-anonymously-on-the-owners-instruction.md) | OpenObserve's own `run.app` URL answers the internet unauthenticated, on explicit repeated owner instruction, amending ADR 0028 decision 5 — the `allUsers` refusal becomes conditional rather than absent so no *other* workload can become anonymous by accident, the order path stays structurally unpublishable, and the cost is zero until the OTLP drain is wired, which is the trigger to revisit |
| [0031](0031-a-vendored-workload-may-take-a-secret-as-an-environment-value.md) | `modules/cloudrun` gains `secret_env`, refused unless `image_source` is `vendored` — OpenObserve's image carries no shell and no `_FILE` symbol for its credential, so the file mount satisfied the rule and was inert; the value still reaches no committed file, plan or state, and the crash-dump exposure it does leave is named rather than solved |
| [0032](0032-telemetry-reaches-a-collector-inside-the-vpc-not-a-service-on-the-internet.md) | Telemetry drains to a vendored, attested collector on a private address, not to a service on the internet — the fast brain is the forcing case, having no egress proxy by design because port 9102 routes to a model API and nothing on the fast path may consult one (ADR 0008); one image closes the fast-brain route, A6's collector and the pull path together, and stops operational history crossing the public internet |
| [0033](0033-openobserve-becomes-authenticated-before-it-holds-telemetry.md) | OpenObserve moves from anonymous to authenticated external access before the first byte of telemetry lands — firing the condition ADR 0030 set for itself; it stays reachable from the internet and stops being reachable by anyone, because an anonymous invoker is anonymous in both directions and a store anyone can write to cannot be evidence for a gate |
| [0034](0034-the-first-market-data-and-prediction-sources.md) | Coinbase for crypto first (keyless, so the transport is proven before any contract), then Alpaca for equities (its market data and its paper brokerage are one account, so vendor expectation and platform behaviour are the same sentence), then Kalshi for prediction — candidates only: `qip-data-finder`'s licensing gate decides, and these terms have not been read against a contract |
| [0035](0035-one-execution-node-in-shadow-mode.md) | One execution node, `us-east4`, shadow mode, dev only — the whole edge plane runs today only under `cargo test`, and one probe answers what is actually open where seven multiplies every first-deployment surprise by seven; partition behaviour is explicitly not tested by it |
