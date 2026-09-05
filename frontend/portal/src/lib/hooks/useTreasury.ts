"use client";

import { request } from "@/lib/api/client";
import { useResource, type Resource } from "./useResource";

/**
 * The ledger plane, read-only: four viewer routes and the shapes they answer.
 *
 * `GET /ledger/users`, `GET /wallet`, `GET /corridors` and `GET /transfer-gate`
 * are the platform's own account of who holds capital, where it is observed,
 * where it may go, and what the veto-only gate last said. This console reads
 * them and renders them. It does not compute a balance, sum an inflow into an
 * availability, decide a corridor stage or run a gate check — every figure on
 * the treasury pages is a field one of these routes answered.
 *
 * Nothing in this file is a write. There is no fetcher here for a proposal,
 * an approval, a signature or a transfer, and `client.ts` declares none: the
 * gateway refuses any non-GET this console does not declare before the
 * credential is read (`tests/boundary.spec.ts`). ADR 0021 refuses the half of
 * the treasury by which capital leaves, ADR 0023 keeps that in force, and the
 * routes themselves answer 405 to anything but GET.
 *
 * Shapes are transcribed field for field from
 * `backend/crates/apps/qip-api/ROUTES-LEDGER.md` and the view structs in
 * `backend/crates/apps/qip-api/src/ledger_views.rs`: snake_case keys, every
 * money figure a string carrying the platform's exact `Decimal` text, every
 * timestamp RFC 3339 UTC or `null`, absence stated with a boolean and a
 * reason rather than zero-filled.
 */

const POLL_MS = 15_000;

// --- conventions every body follows --------------------------------------------

/** A `qip_core::Decimal`, serialised as its exact decimal string. Never parsed here. */
export type DecimalString = string;

/** A `qip_core::Timestamp`, serialised as RFC 3339 UTC. */
export type Rfc3339 = string;

/**
 * What every treasury body carries first. `posture` is the platform's own
 * literal, rendered as it came so a body that ever said something else would
 * be shown saying it.
 */
export interface TreasuryBody {
  readonly posture: string;
  readonly served_at: Rfc3339;
}

// --- GET /ledger/users ---------------------------------------------------------

/** `CapabilityView`: whether, and the basis of a grant or the input that refused. */
export interface Capability {
  readonly granted: boolean;
  readonly reason: string;
}

export interface Entitlement {
  readonly family: string;
  readonly role: string;
  readonly evaluated_at: Rfc3339;
  readonly can_view: Capability;
  readonly can_invest: Capability;
  /** `granted` is always `false`: the platform's type has no granted arm. */
  readonly can_withdraw: Capability;
}

export interface PermittedFamilies {
  /** `true` means every family; otherwise `families` lists the only ones. */
  readonly any: boolean;
  readonly families: readonly string[];
}

export interface Mandate {
  readonly capital: DecimalString;
  readonly currency: string;
  readonly risk_tolerance: DecimalString;
  readonly liquidity_floor: DecimalString;
  /** `capital - liquidity_floor`, as the platform computed it. */
  readonly investable: DecimalString;
  readonly exploration_share: DecimalString;
  readonly jurisdiction: string;
  readonly permitted_families: PermittedFamilies;
}

export interface ExpectedInflow {
  readonly reference: string;
  readonly amount: DecimalString;
  readonly declared_at: Rfc3339;
}

/** One `(strategy, currency)` book of one user. */
export interface Balance {
  readonly strategy: string;
  readonly currency: string;
  readonly settled: DecimalString;
  readonly reserved: DecimalString;
  /** `settled - reserved`. Expected inflows are not in it. */
  readonly available: DecimalString;
  /** Visible and never added to anything. */
  readonly expected_inflows_total: DecimalString;
  readonly expected_inflows: readonly ExpectedInflow[];
  readonly entries: number;
  readonly last_entry_at: Rfc3339 | null;
}

export interface LedgerUser {
  readonly user_id: string;
  readonly mandate: Mandate;
  readonly balances: readonly Balance[];
  readonly entitlements: readonly Entitlement[];
  /** Set when `entitlements` is empty, saying why. */
  readonly entitlements_note: string | null;
}

export interface LedgerUsers extends TreasuryBody {
  readonly evaluated_as_role: string;
  readonly products: readonly string[];
  readonly fills_journalled: number;
  readonly users: readonly LedgerUser[];
}

// --- GET /wallet -----------------------------------------------------------------

export type Provenance = "read_only_api_key" | "watch_only_address" | "view_key" | "statement";

export interface Holding {
  readonly venue: string;
  readonly asset: string;
  readonly observed_quantity: DecimalString;
  readonly observed_at: Rfc3339;
  readonly provenance: Provenance | string;
  readonly ledger_expected: DecimalString;
}

/** The fabric's `ReconciliationAlert`, complete in itself. */
export interface ReconciliationAlert {
  readonly venue_asset: { readonly venue: string; readonly asset: string };
  readonly cause: "delta_beyond_tolerance" | "unrecorded_by_ledger" | string;
  readonly expected: DecimalString;
  readonly observed: DecimalString;
  readonly delta: DecimalString;
  readonly tolerance: DecimalString;
  readonly observed_at: Rfc3339;
  readonly provenance: Provenance | string;
  readonly message: string;
}

/** The fabric's `ReconciliationOutcome`, tagged on `outcome`. A halt instructs; nothing auto-corrects. */
export type ReconciliationOutcome =
  | { readonly outcome: "reconciled"; readonly venue: string; readonly asset: string; readonly delta: DecimalString }
  | {
      readonly outcome: "halt";
      readonly venue: string;
      readonly asset: string;
      readonly delta: DecimalString;
      readonly alert: ReconciliationAlert;
    };

export interface Wallet extends TreasuryBody {
  /** Whether a wallet exists in this process. */
  readonly assembled: boolean;
  /** Why not, when `assembled` is `false`. */
  readonly reason: string | null;
  readonly as_of: Rfc3339 | null;
  readonly holdings: readonly Holding[];
  readonly reconciliation: {
    readonly outcomes: readonly ReconciliationOutcome[];
    /** The platform's own count of `outcomes` whose `outcome` is `"halt"`. */
    readonly halted_venue_assets: number;
  };
}

// --- GET /corridors -------------------------------------------------------------

export type CorridorStage =
  | "proposed"
  | "reviewed"
  | "signed"
  | "time_delayed"
  | "active"
  | "suspended"
  | "revoked";

export interface CorridorCaps {
  readonly max_per_transfer: DecimalString;
  readonly max_per_hour: DecimalString;
  readonly max_per_day: DecimalString;
  readonly max_cumulative: DecimalString;
  readonly min_interval_seconds: number;
  /** A half-open window `[start, end)` in whole UTC hours. */
  readonly permitted_hours: { readonly start: number; readonly end: number };
}

export interface Corridor {
  readonly id: string;
  readonly source: { readonly region: string; readonly currency: string; readonly venue: string };
  readonly source_class: string;
  readonly kind: string;
  readonly destination: { readonly asset: string; readonly address: string };
  readonly caps: CorridorCaps;
  readonly purpose: string;
  readonly stage: CorridorStage | string;
  readonly proposed_by: string;
  readonly proposed_at: Rfc3339;
  readonly reviewed_by: string | null;
  readonly reviewed_at: Rfc3339 | null;
  /** Whether a signature record covers the destination and every cap. */
  readonly signed: boolean;
  readonly activation_at: Rfc3339 | null;
}

export type DestinationStatus = "proposed" | "verified" | "signed" | "revoked";

export interface Destination {
  readonly asset: string;
  readonly address: string;
  readonly status: DestinationStatus | string;
  readonly proposed_by: string;
  readonly proposed_at: Rfc3339;
  /** The registry's own instant; present once signed. */
  readonly usable_from: Rfc3339 | null;
}

/** A registry the process may or may not hold. */
export interface Registry<T> {
  readonly held: boolean;
  readonly reason: string | null;
  readonly records: readonly T[];
}

export interface Corridors extends TreasuryBody {
  readonly corridors: Registry<Corridor>;
  readonly destinations: Registry<Destination>;
}

// --- GET /transfer-gate ------------------------------------------------------------

export type GateCheckName =
  | "corridor_authority"
  | "caps"
  | "minimum_interval"
  | "stated_purpose"
  | "source_balance"
  | "velocity_and_anomaly"
  | "kill_switch";

export interface GateCheck {
  /** 1-based position in assessment order. */
  readonly order: number;
  readonly name: GateCheckName | string;
  /** Whether §37.3 pairs a veto by this check with an alert to a person. */
  readonly alerts: boolean;
}

/** An assessment, were one ever recorded. */
export interface GateAssessment {
  readonly assessed_at: Rfc3339;
  readonly outcome: "approved" | "vetoed" | string;
  readonly check: string | null;
  readonly reason: string | null;
  readonly alert: boolean;
}

export interface KillSwitch {
  readonly halted: boolean;
  readonly halted_scopes: readonly string[];
  readonly tripped_by: string | null;
  readonly reason: string | null;
  readonly tripped_at: Rfc3339 | null;
}

export interface TransferGate extends TreasuryBody {
  readonly checks: readonly GateCheck[];
  /** `null` until something assesses an intent — an absence, not a pass. */
  readonly last_assessment: GateAssessment | null;
  readonly kill_switch: KillSwitch;
  /** Constant `false`: the gate cannot move anything. */
  readonly executes: boolean;
  readonly note: string;
}

// --- fetchers ----------------------------------------------------------------------

/**
 * The four reads, and the only four. Kept here rather than in `platform` so a
 * reviewer of the treasury surface sees every request it can make in one
 * place, and sees that each is a GET.
 */
export const treasury = {
  ledgerUsers: (signal?: AbortSignal) => request<LedgerUsers>("/ledger/users", signal ? { signal } : {}),
  wallet: (signal?: AbortSignal) => request<Wallet>("/wallet", signal ? { signal } : {}),
  corridors: (signal?: AbortSignal) => request<Corridors>("/corridors", signal ? { signal } : {}),
  transferGate: (signal?: AbortSignal) => request<TransferGate>("/transfer-gate", signal ? { signal } : {}),
} as const;

export function useLedgerUsers(): Resource<LedgerUsers> {
  return useResource<LedgerUsers>(treasury.ledgerUsers, {
    key: "treasury-ledger-users",
    label: "GET /ledger/users",
    intervalMs: POLL_MS,
  });
}

export function useWallet(): Resource<Wallet> {
  return useResource<Wallet>(treasury.wallet, {
    key: "treasury-wallet",
    label: "GET /wallet",
    intervalMs: POLL_MS,
  });
}

export function useCorridors(): Resource<Corridors> {
  return useResource<Corridors>(treasury.corridors, {
    key: "treasury-corridors",
    label: "GET /corridors",
    intervalMs: POLL_MS,
  });
}

export function useTransferGate(): Resource<TransferGate> {
  return useResource<TransferGate>(treasury.transferGate, {
    key: "treasury-transfer-gate",
    label: "GET /transfer-gate",
    intervalMs: POLL_MS,
  });
}

// --- helpers the pages share ---------------------------------------------------------

/** Render a whole number of seconds in hours, minutes or seconds. */
export function formatSeconds(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "—";
  if (seconds >= 3_600) {
    const hours = seconds / 3_600;
    return Number.isInteger(hours) ? `${hours}h` : `${hours.toFixed(2)}h`;
  }
  if (seconds >= 60) {
    const minutes = seconds / 60;
    return Number.isInteger(minutes) ? `${minutes}m` : `${minutes.toFixed(1)}m`;
  }
  return `${seconds}s`;
}
