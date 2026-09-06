"use client";

import { useEffect, useState } from "react";
import { request, type ApiResponse } from "@/lib/api/client";
import { useResource, type Resource } from "./useResource";

/**
 * Venue registrations: what each source demands before the platform reads
 * it, who has registered, and the one write by which an operator records
 * that they did.
 *
 * # Two reads, because the platform split the answer by authority
 *
 * `GET /registrations` is the platform's own account, transcribed field for
 * field from the view structs in `qip-api/src/registration_views.rs`: a
 * requirement declared per source from the venue's own documentation (or
 * `null` where none was declared), a standing that is `keyless`,
 * `registered` by a named person, or `pending` with who must register and
 * why, and the terms reference the operator is told to read. This console
 * renders those and computes none of them.
 *
 * It no longer carries a credential slot. `SourceStandingView` is what
 * `Role::Viewer` is answered with, and the slot a credential is read under —
 * the deployment variable, the `gcloud` line that fills it, any companion
 * variable, and the variable a `registered` record itself names — moved to
 * `GET /registrations/slots` at `Role::Operator`
 * (`ROUTES-REGISTRATIONS.md`, "Why the read is two routes"). A slot is not a
 * fact about a venue; it names where a credential lives in this deployment's
 * secret store, and it was being served to a viewer credential. This console
 * is what found that, and this hook is what stopped describing the shape it
 * found.
 *
 * {@link useRegistrationSlots} is the second read, and {@link slotAccess} is
 * what a page renders from it. **The read is attempted rather than assumed
 * away, and that is the decision this comment exists to justify.** This
 * process cannot know its own authority: `upstream.ts` reads `QIP_API_TOKEN`
 * as an opaque string and attaches it, and nothing in the browser or in this
 * server can tell which role the platform will map it to. A panel that
 * refused *without asking* would be a second claim about a fact the platform
 * owns, and the failure that produces is specific: the day a deployment
 * mounts a different token, the console goes on telling an operator they may
 * not see something they are in fact being served. So the console asks, and
 * renders the platform's own answer either way.
 *
 * What the answer is today, in every committed configuration: refused.
 * `scripts/deploy-frontends.sh` mounts `qip-token-viewer-<env>` at the path
 * `QIP_API_TOKEN_FILE` names, the console's service account is granted
 * `secretAccessor` on `qip-token-viewer` and nothing else
 * (`infrastructure/terraform/modules/secrets/main.tf`, "The console reads the
 * platform as `viewer`, and holds no other platform credential"), and ADR
 * 0018 decided it: "**The console authenticates as `viewer`.** … Viewer is
 * the whole entitlement." So the slots panel renders a refusal naming who may
 * read the route and how — never a blank panel, and never an invented
 * endpoint standing in for one the console may not call.
 *
 * `POST /registrations/{source_id}/approve` is the fourth write this console
 * declares (`endpoints.ts`). It records a registration a person made — the
 * terms they read and the variable they put the key under — and the platform
 * creates no account and reads no venue on the strength of it. The body
 * carries a variable *name*, never a value: the platform refuses a key-shaped
 * secret with a 400 naming the field, and that refusal is rendered as it
 * came rather than retried.
 *
 * The write goes through `request` exactly as the kill switch does: the
 * gateway attaches the deployment's credential, and the platform's 403 for a
 * credential without the operator role comes back as `denied`. The session's
 * own role is read separately (`useSessionIdentity`) so a viewer sees the
 * control refused before the platform is asked, with the reason.
 */

const POLL_MS = 15_000;

/** A `qip_core::Timestamp`, serialised as RFC 3339 UTC. */
export type Rfc3339 = string;

/** `RegistrationRequirement`, snake_case as serde writes it. */
export type RegistrationRequirement =
  | "keyless"
  | "self_service_api_key"
  | "account"
  | "account_with_identity_verification";

/**
 * `StandingSummaryView`, tagged on `standing` — `#[serde(tag = "standing")]`
 * in `qip-api/src/registration_views.rs`, so the object under a source's
 * `standing` key carries its own `standing` discriminator.
 *
 * There is no `secret` on the `registered` arm. The platform dropped it from
 * the viewer's list deliberately, and the reason is worth keeping here: while
 * it was on this arm the disclosure was closed only until somebody
 * registered, because before an approval a viewer saw no slot for a source
 * and the moment an operator approved one the same viewer saw
 * `QIP_ALPACA_API_SECRET_KEY` in the standing instead of in the row. The slot
 * the *record* names is on {@link RegistrationSlotStanding}.
 */
export type RegistrationStanding =
  | { readonly standing: "keyless" }
  | {
      readonly standing: "registered";
      readonly operator: string;
      readonly terms_read_at: Rfc3339;
    }
  | { readonly standing: "pending"; readonly who_must_register: string; readonly reason: string };

/** `SourceStandingView`, field for field: one row of `GET /registrations`. */
export interface RegistrationSource {
  readonly source_id: string;
  /**
   * The declared requirement, or `null` when the registry declares none —
   * which the standing then reports as pending, because an unasked question
   * is not a keyless source.
   */
  readonly requirement: RegistrationRequirement | string | null;
  readonly standing: RegistrationStanding;
  /** The venue's terms: a URL or a document name, or `null` where none is declared. */
  readonly terms: string | null;
}

/** `RegistrationStandingsView`: `GET /registrations`. */
export interface Registrations {
  /** The platform's own literal, rendered as it came. */
  readonly posture: string;
  readonly served_at: Rfc3339;
  /** Every source in the finder's catalogue, in catalogue order. */
  readonly sources: readonly RegistrationSource[];
}

// --- the operator's list: GET /registrations/slots ------------------------------

/**
 * `StandingView`: the standing as the operator route answers it, with the
 * deployment variable the registration record names.
 *
 * It can differ from the row's `secret_slot` — a registration mounted from
 * the committed file need not agree with the shipped manifest — which is
 * exactly why both are rendered rather than one taken for the other.
 */
export type RegistrationSlotStanding =
  | { readonly standing: "keyless" }
  | {
      readonly standing: "registered";
      readonly operator: string;
      readonly terms_read_at: Rfc3339;
      /** The deployment variable name the credential is read from. Never a value. */
      readonly secret: string;
    }
  | { readonly standing: "pending"; readonly who_must_register: string; readonly reason: string };

/** A further variable the manifest reads beside the primary one, with its command. */
export interface SecretSlot {
  readonly variable: string;
  readonly secret_command: string;
}

/** `SourceRegistrationView`, field for field: one row of `GET /registrations/slots`. */
export interface RegistrationSlotSource {
  readonly source_id: string;
  readonly requirement: RegistrationRequirement | string | null;
  readonly standing: RegistrationSlotStanding;
  readonly terms: string | null;
  /** The variable the connector manifest reads the credential from, or `null` for a keyless source. */
  readonly secret_slot: string | null;
  /** The one-line command that puts a version behind `secret_slot`, or `null`. */
  readonly secret_command: string | null;
  /** Every further variable the manifest reads, each with its command. Empty when there is none. */
  readonly companion_secret_slots: readonly SecretSlot[];
}

/** `RegistrationsView`: `GET /registrations/slots`. */
export interface RegistrationSlots {
  readonly posture: string;
  readonly served_at: Rfc3339;
  readonly sources: readonly RegistrationSlotSource[];
}

/**
 * Narrow an operator standing to the viewer's, mirroring
 * `StandingSummaryView::of` rather than deriving the fact a second time.
 *
 * Used where the approval's answer (an operator shape) has to sit on a card
 * built from the viewer's list. Written as an exhaustive switch that names
 * `secret` nowhere in its output, so a slot cannot arrive on a viewer card by
 * a spread that was open-ended.
 */
export function viewerStanding(standing: RegistrationSlotStanding): RegistrationStanding {
  switch (standing.standing) {
    case "keyless":
      return { standing: "keyless" };
    case "registered":
      return {
        standing: "registered",
        operator: standing.operator,
        terms_read_at: standing.terms_read_at,
      };
    case "pending":
      return {
        standing: "pending",
        who_must_register: standing.who_must_register,
        reason: standing.reason,
      };
  }
}

/**
 * `ApprovalView`: what the approval answers — the source's standing after
 * the record was journalled, not the whole source. The page merges it into
 * the card it already holds until the next list lands.
 *
 * An operator standing, because the approval route is `Role::Operator` and
 * its 200 carries the `secret` the record names (`ROUTES-REGISTRATIONS.md`).
 */
export interface Approval {
  readonly posture: string;
  readonly served_at: Rfc3339;
  readonly source_id: string;
  readonly standing: RegistrationSlotStanding;
}

/** The body `POST /registrations/{source_id}/approve` takes. */
export interface ApproveRegistrationBody {
  readonly terms: string;
  /** A variable name, `SCREAMING_SNAKE_CASE`. The platform refuses anything key-shaped. */
  readonly secret: string;
}

// --- the read and the write ----------------------------------------------------

export const registrations = {
  list: (signal?: AbortSignal) =>
    request<Registrations>("/registrations", signal ? { signal } : {}),
  /**
   * The credential slots, at the operator role. A separate route and not a
   * query parameter on the one above: the platform's route table states each
   * route's authority in one place so a security review reads the table
   * instead of the handlers, and a body that varied by caller would make that
   * table untrue.
   */
  slots: (signal?: AbortSignal) =>
    request<RegistrationSlots>("/registrations/slots", signal ? { signal } : {}),
  /**
   * The source id is one path segment. The gateway refuses a segment that is
   * not routable before the credential is read, and `declaresWrite` matches
   * exactly one segment in that position; a source id carrying a slash would
   * be refused there rather than approving something else.
   */
  approve: (sourceId: string, body: ApproveRegistrationBody): Promise<ApiResponse<Approval>> =>
    request<Approval>(`/registrations/${encodeURIComponent(sourceId)}/approve`, {
      method: "POST",
      body,
    }),
} as const;

export function useRegistrations(): Resource<Registrations> {
  return useResource<Registrations>(registrations.list, {
    key: "registrations",
    label: "GET /registrations",
    intervalMs: POLL_MS,
  });
}

/**
 * The credential slots, read once on mount rather than polled.
 *
 * Once, because what this route adds is a fact about the deployment's secret
 * store — which variable a manifest reads and the line that fills it — and
 * that does not change between two polls of a page. Polling it would also
 * mean re-asking a question this console's credential is refused, every
 * fifteen seconds, for as long as a tab is open: a steady stream of 403s in
 * the platform's log that says nothing the first one did not.
 *
 * `refresh()` is still available and the registrations page calls it after an
 * approval, because the `secret` a record names is the one field here that an
 * approval does change.
 */
export function useRegistrationSlots(): Resource<RegistrationSlots> {
  return useResource<RegistrationSlots>(registrations.slots, {
    key: "registration-slots",
    label: "GET /registrations/slots",
  });
}

// --- what a page renders from the second read ----------------------------------

/** The route, written as an operator reads it in the platform's route table. */
export const SLOTS_ROUTE = "GET /api/v1/registrations/slots";

/**
 * Who may read {@link SLOTS_ROUTE}, and how — the sentence a refused panel
 * shows in place of the slots.
 *
 * It names a role, a reason and a way to get the fact anyway, because a panel
 * that said only "refused" would leave an operator with the credential in
 * their hand and no idea that it is the console rather than the platform
 * withholding it. `qip registrations` is not an invented alternative: it calls
 * `registration_views::registrations` directly against a local platform
 * (`qip-cli/src/main.rs`, `registrations_command`), so it prints this list
 * with no HTTP role in the way at all.
 */
export const SLOTS_REFUSAL =
  "The credential this console holds may not read it. The platform serves that route at the " +
  "operator role, and this console authenticates as viewer and holds no other platform " +
  "credential — its service account is granted the viewer token and nothing else, which is " +
  "ADR 0018's decision rather than an oversight. So the deployment variable each source's " +
  "credential is read under, and the one command that puts a version behind it, are not this " +
  "browser's to receive. An operator holding an operator credential reads them from the route " +
  "itself, or runs `qip registrations` on a machine that has the platform's configuration, " +
  "which prints the same list without asking anyone for a role.";

/**
 * The second read, as a page has to render it.
 *
 * Four arms and not two, because "the console may not see this", "nobody has
 * answered yet", "the platform could not be reached" and "the platform served
 * them" are four different things to put on a screen, and a panel that
 * collapsed the first three into an empty column would report a source as
 * having no credential slot when what happened is that nobody was asked.
 */
export type SlotAccess =
  | { readonly status: "loading" }
  | {
      readonly status: "available";
      readonly bySource: ReadonlyMap<string, RegistrationSlotSource>;
      readonly servedAt: Rfc3339;
    }
  /** The platform refused this console's credential. Its own words are `detail`. */
  | { readonly status: "refused"; readonly detail: string; readonly status_code: number }
  /** Anything else: unreachable, no such route, or an error the platform named. */
  | { readonly status: "unavailable"; readonly detail: string };

export function slotAccess(resource: Resource<RegistrationSlots>): SlotAccess {
  const { outcome } = resource;
  if (outcome === null) return { status: "loading" };
  switch (outcome.kind) {
    case "ok":
      return {
        status: "available",
        bySource: new Map(outcome.data.sources.map((source) => [source.source_id, source])),
        servedAt: outcome.data.served_at,
      };
    case "denied":
      return { status: "refused", detail: outcome.detail, status_code: outcome.status };
    case "unavailable":
      return { status: "unavailable", detail: outcome.reason };
    case "missing":
    case "unreachable":
    case "error":
      return { status: "unavailable", detail: outcome.detail };
  }
}

// --- who is signed in, and whether they may approve ---------------------------

/**
 * The session as `/api/auth/session` projects it: the name to approve as and
 * the roles the sealed cookie carries. Read once on mount, the same way the
 * dashboard greeting and the account menu read it.
 */
export type SessionIdentity =
  | { readonly status: "loading" }
  /** An open deployment, or no cookie: there is no named person to approve as. */
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

/**
 * Whether this session may approve, and if not, why — in words the page
 * shows beside the disabled control. Not a permission: the platform decides
 * with its own credential and answers 403 to a credential without the role.
 * This is the reason a person sees before asking, so a viewer is not told
 * "the credential was refused" about a control they were never entitled to.
 */
export function approvalPermission(identity: SessionIdentity): { readonly allowed: boolean; readonly reason: string } {
  switch (identity.status) {
    case "loading":
      return { allowed: false, reason: "reading who is signed in" };
    case "unauthenticated":
      return {
        allowed: false,
        reason:
          "no one is signed in to this console, so there is no named operator to register as; " +
          "an approval must carry the person who read the terms",
      };
    case "authenticated":
      if (identity.roles.includes(OPERATOR_ROLE)) return { allowed: true, reason: "" };
      return {
        allowed: false,
        reason:
          `your session holds the ${describeRoles(identity.roles)} role and not operator; ` +
          "approving a registration needs the operator role, which an operator grants with an audit trail",
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
    displayName: typeof fields.displayName === "string" && fields.displayName.length > 0 ? fields.displayName : null,
    roles,
  };
}
