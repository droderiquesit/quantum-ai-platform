/**
 * The shapes the platform's REST surface actually returns.
 *
 * These are transcribed from the handler table in
 * `backend/crates/apps/qip-api/src/routes.rs`, field for field. Where the backend
 * answers a request with an explicit absence rather than data, that is modelled
 * as {@link Unavailable} instead of an optional field, because "no data and
 * here is why" is a different answer from "zero".
 */

/** A subject this deployment has nothing behind, and the reason it does not. */
export interface Unavailable {
  readonly subject: string;
  readonly available: false;
  readonly reason: string;
}

/** A field that is either real data or a stated absence. */
export type Section<T> = T | Unavailable;

export function isUnavailable(value: unknown): value is Unavailable {
  return (
    typeof value === "object" &&
    value !== null &&
    "available" in value &&
    (value as { available: unknown }).available === false
  );
}

export type AutonomyLevel = string;

export interface Health {
  readonly status: string;
  readonly halted: boolean;
  readonly autonomy: AutonomyLevel;
  readonly live_capable: boolean;
  readonly reconciliation_breaks: number;
}

export interface MeshStatus {
  readonly served: boolean;
  readonly error?: string;
  readonly cells_served?: number;
  readonly deltas_absorbed?: number;
  readonly envelopes_dispatched?: number;
  readonly inbox_depth?: number;
}

export interface SystemStatus {
  readonly autonomy: AutonomyLevel;
  readonly configured_autonomy: AutonomyLevel;
  readonly ceiling: AutonomyLevel;
  readonly live_capable: boolean;
  readonly halted: boolean;
  readonly halted_scopes: readonly string[];
  readonly cycles: number;
  readonly events: number;
  /** null when this deployment archives nothing — not zero. */
  readonly archived: number | null;
  readonly mesh: MeshStatus;
}

export interface SystemMetrics {
  readonly cycles: number;
  readonly events_logged: number;
  readonly opportunities_queued: number;
  readonly proposals: number;
  readonly orders: number;
  readonly fills: number;
  readonly refusals: number;
  readonly live_fills: boolean;
}

export interface GovernanceFinding {
  readonly severity: "error" | "warning";
  readonly rule: string;
  readonly detail: string;
  readonly agents: readonly string[];
}

export interface Governance {
  readonly agents: number;
  readonly findings: readonly GovernanceFinding[];
}

export interface Portfolio {
  readonly proposals: number;
  readonly orders: number;
  readonly fills: number;
  readonly paper_only: boolean;
}

export interface Opportunity {
  readonly id: string;
  readonly headline: string;
  readonly score: number;
  readonly confidence: number;
  readonly detectors: readonly string[];
}

export interface Opportunities {
  readonly opportunities: readonly Opportunity[];
}

export interface Proposal {
  readonly id: string;
  readonly status: string;
  readonly legs: number;
  readonly gross: number;
  readonly turnover: number;
  readonly rationale: string;
}

export interface Proposals {
  readonly proposals: readonly Proposal[];
}

export interface Order {
  readonly id: string;
  readonly instrument: string;
  readonly side: string;
  /** A decimal rendered as a string upstream; never parsed into a float here. */
  readonly quantity: string;
  readonly state: string;
  readonly filled: string;
  readonly simulated: boolean;
}

export interface Orders {
  readonly orders: readonly Order[];
  readonly refusals: number;
  readonly reconciliation_breaks: readonly string[];
}

export interface Fill {
  readonly order: string;
  readonly instrument: string;
  readonly side: string;
  readonly quantity: string;
  readonly price: string;
  readonly venue: string;
  readonly simulated: boolean;
}

export interface Fills {
  readonly fills: readonly Fill[];
  readonly any_live_fill: boolean;
}

export interface AgentManifest {
  readonly id: string;
  readonly name: string;
  readonly role: string;
  readonly owner: string;
  readonly purpose: string;
  readonly capabilities: readonly string[];
}

export interface Agents {
  readonly agents: readonly AgentManifest[];
}

export interface AutonomyChange {
  readonly at: number;
  readonly from: string;
  readonly to: string;
  readonly operator: string;
  readonly reason: string;
}

export interface Autonomy {
  readonly level: AutonomyLevel;
  readonly ceiling: AutonomyLevel;
  readonly live: boolean;
  readonly history: readonly AutonomyChange[];
}

export interface SystemView {
  readonly autonomy: AutonomyLevel;
  readonly ceiling: AutonomyLevel;
  readonly live: boolean;
  readonly halted: boolean;
  readonly halted_scopes: readonly string[];
  readonly cycles: number;
  readonly events_logged: number;
  readonly chain_intact: boolean;
  readonly chain_broken_at: number | null;
}

export interface CellObservation {
  readonly cell: string;
  readonly reported_at: string;
  readonly age: string;
  readonly stale: boolean;
  readonly halted: boolean;
  readonly positions: number;
  readonly strategies: number;
  readonly reconciliation_breaks: number;
  readonly gross: string;
  readonly net: string;
}

export interface Regions {
  readonly freshness_bound: string;
  readonly cells: readonly CellObservation[];
}

export interface StrategyCandidate {
  readonly id: string;
  readonly cell: string;
  readonly venue: string;
  readonly stage: string;
  readonly holds_capital: boolean;
  readonly registered_at: string;
}

export interface Strategies {
  readonly strategies: readonly StrategyCandidate[];
}

export interface Models {
  readonly registry: Unavailable;
  readonly observed_use: {
    readonly agent_runs: number;
    readonly model_calls: number;
    readonly tokens: number;
    readonly cost_micros: number;
  };
}

export type EnvelopeUse =
  | { readonly reported: true; readonly gross_committed: string; readonly orders_sent: number }
  | { readonly reported: false; readonly reason: string };

export interface CapitalEnvelope {
  readonly cell: string;
  readonly strategy: string;
  readonly gross_limit: string;
  readonly expires_at: string;
  readonly used: EnvelopeUse;
}

export interface CapitalRecall {
  readonly cell: string;
  readonly strategy: string;
  readonly reason: string;
  readonly detail: string;
  readonly issued_at: string;
  readonly acknowledge_by: string;
  readonly gross_recalled: string;
  readonly backstop_expiry: string;
}

export interface Capital {
  readonly bounds: {
    readonly total_budget: string;
    readonly per_strategy: string;
    readonly per_cell: string;
    readonly per_venue: string;
  };
  readonly envelopes: readonly CapitalEnvelope[];
  readonly outstanding_recalls: readonly CapitalRecall[];
}

export interface ExposureBucket {
  readonly axis: string;
  readonly bucket: string;
  readonly gross: string;
  readonly net: string;
  readonly share: number;
  readonly limit: number;
  readonly breached: boolean;
}

export interface ConcentrationFinding {
  readonly axis: string;
  readonly bucket: string;
  readonly gross: string;
  readonly share: number;
  readonly limit: number;
}

export interface Risk {
  readonly exposure: Section<{ readonly available: true; readonly buckets: readonly ExposureBucket[] }>;
  readonly concentrations: Section<{
    readonly available: true;
    readonly findings: readonly ConcentrationFinding[];
  }>;
  readonly kill_switch: {
    readonly halted: boolean;
    readonly halted_scopes: readonly string[];
    readonly tripped_by: string;
    readonly reason: string;
    readonly clearances: number;
  };
  readonly limit_utilisation: Unavailable;
  readonly tail_risk: Unavailable;
}

export interface Quantum {
  readonly jobs: Unavailable;
  readonly routing: {
    readonly provider: string;
    readonly classical_baseline: string;
    readonly note: string;
  };
}

export interface CycleStage {
  readonly stage: string;
  readonly ran: boolean;
  readonly produced: number;
  readonly detail: string;
  readonly problems: readonly string[];
}

export interface CycleReport {
  readonly cycle: number;
  readonly correlation_id: string;
  readonly halted: boolean;
  readonly traversed_every_stage: boolean;
  readonly archived: number | null;
  readonly archive_error?: string;
  readonly stages: readonly CycleStage[];
  readonly mesh?: unknown;
}

export type KillSwitchResponse =
  | { readonly halted: true; readonly by: string; readonly reason: string }
  | { readonly halted: false; readonly cleared_by: string };

export interface DiscoveryRoute {
  readonly method: string;
  readonly path: string;
  readonly role: string;
  readonly summary: string;
}

export interface Discovery {
  readonly version: string;
  readonly routes: readonly DiscoveryRoute[];
}

/**
 * The subset of the platform's OpenAPI document this console reads.
 *
 * Only the fields it renders are modelled. The document is generated from the
 * server's own route table at the moment it is requested, so it describes the
 * surface the running process is actually serving — which is why this console
 * reads it rather than rendering the static table in `endpoints.ts`. When the
 * two disagree, the document is right and the table is stale.
 */
export interface OpenApiOperation {
  readonly operationId?: string;
  readonly summary?: string;
  readonly description?: string;
  readonly tags?: readonly string[];
  readonly "x-required-role"?: string;
}

export interface OpenApiDocument {
  readonly openapi: string;
  readonly info: {
    readonly title: string;
    readonly version: string;
    readonly description?: string;
  };
  readonly paths: Readonly<Record<string, Readonly<Record<string, OpenApiOperation>>>>;
}

// --- research routes ---------------------------------------------------------
//
// Transcribed from `predictions`, `correlation`, `backtests` and `regimes` in
// `qip-api/src/routes.rs`, and checked against a running process on
// 2026-09-04. Where the route serves a stated absence inside an otherwise
// available body — the calibration before a claim resolves, the equity curve
// that is never kept — it is modelled as `Unavailable`, for the reason the
// file header gives: an absent report and a report of zero are different
// answers, and a page that cannot tell them apart will draw the second.

/** One falsifiable claim the REASON stage wrote down, as LEARN has graded it. */
export interface RecordedPrediction {
  readonly hypothesis: string;
  readonly cycle: number;
  readonly statement: string;
  /** The platform's own series the claim settles on, e.g. `close:obj-AAA`. */
  readonly metric: string;
  /** "unstated" is a value: the record carried no claim, and no direction. */
  readonly direction: "up" | "down" | "unstated";
  /** In [0, 1], or null where the record carried no claim. */
  readonly confidence: number | null;
  readonly expected_move_bps: number | null;
  readonly horizon_seconds: number;
  readonly made_at: string;
  readonly resolves_at: string;
  /** "undetermined" is a resolved claim the series could not settle either way. */
  readonly state: "open" | "held" | "failed" | "undetermined";
  readonly scored_at: string | null;
}

export interface CalibrationReport {
  readonly available: true;
  readonly evaluations_in_window: number;
  readonly material: boolean;
  /** The LEARN stage's own report, serialised as it holds it. */
  readonly report: Readonly<Record<string, unknown>> & {
    readonly evaluated?: number;
    readonly brier_score?: number;
  };
}

export interface Predictions {
  readonly as_of_cycle: number;
  /** The working set's bound, so `held` can be read against it. */
  readonly window: number;
  readonly held: number;
  readonly open: number;
  readonly resolved: number;
  /** Keyed by instrument, in the platform's key order. */
  readonly instruments: Readonly<Record<string, { readonly predictions: readonly RecordedPrediction[] }>>;
  readonly calibration: Section<CalibrationReport>;
}

export interface CorrelationExclusion {
  readonly instrument: string;
  readonly closes: number;
  readonly reason: string;
}

export interface CorrelationUndefinedPair {
  readonly a: string;
  readonly b: string;
  readonly reason: string;
}

/** The body when the estimate can be made. Otherwise the route answers `Unavailable`. */
export interface Correlation {
  readonly available: true;
  readonly as_of_cycle: number;
  readonly statistic: string;
  /** How the two series were lined up. Read it: it is not by timestamp. */
  readonly alignment: string;
  readonly window_closes: number;
  readonly window_returns: number;
  readonly minimum_closes: number;
  readonly instruments: readonly string[];
  /** `null` where the coefficient is undefined; never NaN. */
  readonly matrix: Readonly<Record<string, Readonly<Record<string, number | null>>>>;
  readonly excluded: readonly CorrelationExclusion[];
  readonly undefined: readonly CorrelationUndefinedPair[];
}

/** What `/correlation` carries beside `available: false`. */
export interface CorrelationRefusal {
  readonly as_of_cycle?: number;
  readonly minimum_closes?: number;
  readonly instruments_observed?: readonly { readonly instrument: string; readonly closes: number }[];
  readonly excluded?: readonly CorrelationExclusion[];
}

export interface GateFinding {
  readonly check: string;
  readonly passed: boolean;
  readonly detail: string;
}

export interface LedgerMove {
  readonly from: string;
  readonly to: string;
  readonly at: string;
  readonly approver: string | null;
  readonly rationale: string;
  readonly gate: { readonly stage: string; readonly passed: boolean; readonly findings: readonly GateFinding[] } | null;
}

export type HoldoutSubmission =
  | { readonly submitted: false }
  | {
      readonly submitted: true;
      readonly observations: number;
      readonly trials_this_run: number;
      readonly periods_per_year: number;
      readonly cross_validation: {
        readonly folds: number;
        readonly observations: number;
        readonly purged: number;
        readonly embargoed: number;
      };
      readonly leakage_findings: readonly string[];
    };

export type TrialAccount =
  | { readonly on_evidence: false; readonly reason: string }
  | {
      readonly on_evidence: true;
      readonly lifetime: number;
      readonly this_run: number;
      readonly prior: number;
      readonly charged_at: string;
    };

export type HoldoutBand =
  | { readonly present: false; readonly reason: string }
  | {
      readonly present: true;
      readonly sharpe: number;
      readonly lower: number;
      readonly upper: number;
      readonly standard_error: number;
      readonly observations: number;
      readonly periods_per_year: number;
      readonly trials: number;
      readonly method: string;
      readonly as_of: string;
    };

export interface BacktestRecord {
  readonly strategy: string;
  readonly family: string;
  readonly cell: string;
  readonly venue: string;
  readonly stage: string;
  readonly registered_at: string;
  readonly holdout: HoldoutSubmission;
  readonly trial_account: TrialAccount;
  readonly family_lifetime_trials: number | null;
  readonly holdout_band: HoldoutBand;
  readonly ledger: readonly LedgerMove[];
}

export type TrialBook =
  | { readonly attached: false }
  | {
      readonly attached: true;
      readonly durable: boolean;
      readonly families: readonly { readonly family: string; readonly lifetime_trials: number | null }[];
    };

export interface Backtests {
  readonly strategies: readonly BacktestRecord[];
  readonly trial_book: TrialBook;
  /** Always a stated absence today; typed as a section so a future value renders. */
  readonly deflated_sharpe: Section<{ readonly available: true }>;
  /** Always a stated absence: no curve is kept. Typed so the page can only ever say so. */
  readonly equity_curve: Section<{ readonly available: true }>;
}

/** What `/regimes` carries beside `available: false`. */
export interface RegimesRefusal {
  readonly stream_topic?: {
    readonly name: string;
    readonly declared_on: string;
    readonly published: boolean;
  };
}
