"use client";

import { useEffect, useState } from "react";
import { request, type ApiResponse } from "@/lib/api/client";
import { useResource, type Resource } from "./useResource";

/**
 * Venue registrations: what each source demands before the platform reads
 * it, who has registered, and the one write by which an operator records
 * that they did.
 *
 * `GET /registrations` is the platform's own account, transcribed field for
 * field from the view structs in `qip-api/src/registration_views.rs`: a
 * requirement declared per source from the venue's own documentation (or
 * `null` where none was declared), a standing that is `keyless`,
 * `registered` by a named person, or `pending` with who must register and
 * why, the terms reference the operator is told to read, the deployment
 * variable the credential lives under, the one-line command that puts it
 * there, and any companion variable the manifest also reads. This console
 * renders those and computes none of them.
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
 * `StandingView`, tagged on `standing` — `#[serde(tag = "standing")]` in
 * `qip-api/src/registration_views.rs`, so the object under a source's
 * `standing` key carries its own `standing` discriminator.
 */
export type RegistrationStanding =
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

/** `SourceRegistrationView`, field for field. */
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
  /** The variable the connector manifest reads the credential from, or `null` for a keyless source. */
  readonly secret_slot: string | null;
  /** The one-line command that puts a version behind `secret_slot`, or `null`. */
  readonly secret_command: string | null;
  /** Every further variable the manifest reads, each with its command. Empty when there is none. */
  readonly companion_secret_slots: readonly SecretSlot[];
}

/** `RegistrationsView`: `GET /registrations`. */
export interface Registrations {
  /** The platform's own literal, rendered as it came. */
  readonly posture: string;
  readonly served_at: Rfc3339;
  /** Every source in the finder's catalogue, in catalogue order. */
  readonly sources: readonly RegistrationSource[];
}

/**
 * `ApprovalView`: what the approval answers — the source's standing after
 * the record was journalled, not the whole source. The page merges it into
 * the card it already holds until the next list lands.
 */
export interface Approval {
  readonly posture: string;
  readonly served_at: Rfc3339;
  readonly source_id: string;
  readonly standing: RegistrationStanding;
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
