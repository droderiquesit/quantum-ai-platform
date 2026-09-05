"use client";

import { useEffect, useState } from "react";
import { request, type ApiResponse } from "@/lib/api/client";
import { useResource, type Resource } from "./useResource";

/**
 * The ledger plane: four viewer reads, and the one operator write by which an
 * eligibility decision is recorded.
 *
 * `GET /ledger/users`, `GET /wallet`, `GET /corridors` and `GET /transfer-gate`
 * are the platform's own account of who holds capital, where it is observed,
 * where it may go, and what the veto-only gate last said. This console reads
 * them and renders them. It does not compute a balance, sum an inflow into an
 * availability, decide a corridor stage or run a gate check — every figure on
 * the treasury pages is a field one of these routes answered.
 *
 * The one write is `POST /ledger/users/{user}/eligibility`. It records an
 * operator's finding about a person — verified when, where, whether they may
 * have capital put to work, until when, and against which document — and it
 * moves nothing: no instrument, side, quantity or price exists in its body,
 * and no field about capital leaving the platform exists on the record at all.
 * `Eligibility` in `qip-capital` has no `can_withdraw` on purpose, and this
 * file adds none. ADR 0021 refuses the half of the treasury by which capital
 * leaves and ADR 0023 keeps that in force; an eligibility decision is a
 * statement about identity, not a transfer.
 *
 * **The gateway must declare that write before it can reach the platform.**
 * `declaresWrite` in `@/lib/api/endpoints` is the allowlist the gateway
 * consults before the deployment credential is read, and until the `REST`
 * table names this path the gateway answers 405 with
 * `x-qip-gateway: refused` — which the panel renders as the gateway's own
 * refusal rather than the platform's, because those are two different faults
 * with two different remedies.
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

/**
 * `EligibilityView`: whether this user may have capital put to work, as the
 * ledger decided it at request time.
 *
 * The two halves are exclusive and the route says so. When `eligible` is
 * `true` the four terms are the record an operator wrote and `refused` and
 * `reason` are `null`; when it is `false` the terms are `null`, `refused`
 * carries the ledger's stable token — `no_mandate`, `unknown_user`,
 * `revoked`, `not_yet_verified`, `cannot_invest`, `expired`,
 * `jurisdiction_absent` — and `reason` its sentence naming what to do.
 *
 * There is deliberately no field here about withdrawing. `Eligibility` in
 * `qip-capital` omits `can_withdraw` rather than always answering `false`,
 * because a field is a value a transfer path could one day read (ADR 0021).
 */
export interface UserEligibility {
  readonly eligible: boolean;
  readonly verified_at: Rfc3339 | null;
  readonly can_invest: boolean | null;
  /** ISO 3166 alpha-2, as the operator recorded it. */
  readonly jurisdiction: string | null;
  readonly expires_at: Rfc3339 | null;
  /** The ledger's stable token when the verdict is no. */
  readonly refused: string | null;
  readonly reason: string | null;
}

export interface LedgerUser {
  readonly user_id: string;
  readonly mandate: Mandate;
  /**
   * Optional in this type and not in the route.
   *
   * `GET /ledger/users` carries `eligibility` on every row. It is declared
   * optional here so a body from a process serving the shape from before the
   * field existed renders as a verdict this console did not receive, stated
   * in those words, rather than taking the whole treasury page down with it.
   * An absent verdict is never read as an eligible one.
   */
  readonly eligibility?: UserEligibility;
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

// --- POST /ledger/users/{user}/eligibility -------------------------------------------

/**
 * The body the decision takes, tagged on `decision` exactly as
 * `EligibilityDecision` in `qip-capital` is tagged.
 *
 * `reason` is the document reference on both arms: on a grant it is what the
 * operator checked the person against, and on a revocation it is why the
 * finding no longer stands. The registry keeps a revoked record rather than
 * deleting it, so "never verified" and "verified and then revoked" remain
 * two different answers, and the reason is the only account of which.
 *
 * The console composes this body and validates none of it beyond requiring
 * the document reference: an expiry that is not after the verification, a
 * jurisdiction that is not the mandate's, a blank field — each is the
 * platform's refusal to make, with a 400 naming the field, and a console that
 * pre-empted them would be holding a rule the platform is the owner of.
 */
export type EligibilityDecisionBody =
  | {
      readonly decision: "granted";
      readonly verified_at: Rfc3339;
      readonly can_invest: boolean;
      readonly jurisdiction: string;
      readonly expires_at: Rfc3339;
      readonly reason: string;
    }
  | { readonly decision: "revoked"; readonly reason: string };

/**
 * The one write of the treasury surface.
 *
 * The user id is one path segment and is encoded, so an id carrying a slash
 * is refused at the gateway rather than deciding about somebody else. The
 * route answers the user's whole updated row, which the page renders in place
 * of the one it held until the next `GET /ledger/users` lands: the platform's
 * own list wins on every refresh, because a page that kept its memory of a
 * click over the platform's record is the page that shows "eligible" the day
 * the record was lost.
 */
export const ledgerEligibility = {
  decide: (user: string, body: EligibilityDecisionBody): Promise<ApiResponse<LedgerUser>> =>
    request<LedgerUser>(`/ledger/users/${encodeURIComponent(user)}/eligibility`, {
      method: "POST",
      body,
    }),
} as const;

// --- who is signed in, and whether they may decide ------------------------------------

/**
 * The session as `/api/auth/session` projects it: the name to decide as and
 * the roles the sealed cookie carries.
 *
 * Read here rather than shared, which is the same shape the account menu, the
 * dashboard greeting, the marketing call to action and the registrations hook
 * each read for themselves. That is four copies and this is a fifth; a single
 * `useSession` is worth extracting, and doing it inside this change would
 * edit files this change does not own. It is named as follow-up rather than
 * done quietly.
 */
export type SessionIdentity =
  | { readonly status: "loading" }
  /** An open deployment, or no cookie: there is no named person to decide as. */
  | { readonly status: "unauthenticated" }
  | {
      readonly status: "authenticated";
      readonly name: string;
      readonly email: string;
      readonly roles: readonly string[];
    };

export const OPERATOR_ROLE = "operator";

export function useSessionIdentity(): SessionIdentity {
  const [identity, setIdentity] = useState<SessionIdentity>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    fetch("/api/auth/session", { cache: "no-store" })
      .then((response) => (response.ok ? response.json() : null))
      .then((body: unknown) => {
        if (cancelled) return;
        const user = sessionUser(body);
        if (user === null) {
          setIdentity({ status: "unauthenticated" });
          return;
        }
        setIdentity({
          status: "authenticated",
          name: user.displayName ?? user.email,
          email: user.email,
          roles: user.roles,
        });
      })
      .catch(() => {
        if (!cancelled) setIdentity({ status: "unauthenticated" });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return identity;
}

export interface EligibilityPermission {
  readonly allowed: boolean;
  /** Empty when allowed; otherwise the sentence shown beside the disabled panel. */
  readonly reason: string;
}

/**
 * Whether this session may decide, and if not, why — in words the page shows
 * beside the disabled panel.
 *
 * Not a permission. The platform decides with its own credential and answers
 * 403 to one without the operator role; this is the reason a person sees
 * before asking, so a viewer is not told "the credential was refused" about a
 * control they were never entitled to use.
 */
export function eligibilityPermission(identity: SessionIdentity): EligibilityPermission {
  switch (identity.status) {
    case "loading":
      return { allowed: false, reason: "reading who is signed in" };
    case "unauthenticated":
      return {
        allowed: false,
        reason:
          "no one is signed in to this console, so there is no named operator to attest as; " +
          "an eligibility decision must carry the person who verified the identity",
      };
    case "authenticated":
      if (identity.roles.includes(OPERATOR_ROLE)) return { allowed: true, reason: "" };
      return {
        allowed: false,
        reason:
          `your session holds the ${describeRoles(identity.roles)} role and not operator; ` +
          "deciding a user's eligibility needs the operator role, which an operator grants with an audit trail",
      };
  }
}

function describeRoles(roles: readonly string[]): string {
  return roles.length === 0 ? "no" : roles.join(", ");
}

interface SessionUser {
  readonly email: string;
  readonly displayName: string | null;
  readonly roles: readonly string[];
}

function sessionUser(body: unknown): SessionUser | null {
  if (typeof body !== "object" || body === null) return null;
  const record = body as Record<string, unknown>;
  if (record.status !== "authenticated") return null;
  const session = record.session;
  if (typeof session !== "object" || session === null) return null;
  const user = (session as Record<string, unknown>).user;
  if (typeof user !== "object" || user === null) return null;
  const fields = user as Record<string, unknown>;
  if (typeof fields.email !== "string") return null;
  const roles = Array.isArray(fields.roles)
    ? fields.roles.filter((role): role is string => typeof role === "string")
    : [];
  return {
    email: fields.email,
    displayName:
      typeof fields.displayName === "string" && fields.displayName.length > 0
        ? fields.displayName
        : null,
    roles,
  };
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
