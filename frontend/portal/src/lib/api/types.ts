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
