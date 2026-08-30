/**
 * Algorik identity: the seam between a provider and the platform.
 *
 * Two things this file is careful about, both of which are security
 * properties rather than architecture taste:
 *
 * **A token is not an authorization.** The types below deliberately give the
 * browser no way to express "this user may do X". The frontend receives a
 * `Session` describing what to *render*; every action is decided again by the
 * backend, which is the only party that has the roles, the entitlements and
 * the audit trail. A frontend that could grant itself a capability would be a
 * frontend an attacker can grant themselves a capability with.
 *
 * **The provider is replaceable and the surfaces do not know which one ran.**
 * A local development adapter and a Google Identity Platform adapter both
 * implement `IdentityProvider`; the pages import this interface and never a
 * vendor. That is what lets the whole sign-up journey be built and tested
 * before a Google project exists, and it is why no Firebase configuration
 * appears in application source (the brief forbids it, and it would pin the
 * app to one vendor's shape).
 */

/** How a person proved who they are. Recorded for audit and step-up decisions. */
export type AuthMethod = "password" | "google" | "passkey" | "saml" | "oidc" | "development";

export type MfaMethod = "totp" | "sms" | "passkey";

/** What the platform needs before it will act for someone. */
export interface AccountAgreements {
  readonly terms: boolean;
  readonly privacy: boolean;
  readonly riskDisclosure: boolean;
}

export type AccountType = "individual" | "institutional" | "partner" | "developer";

/**
 * The user, as a surface may render them.
 *
 * Deliberately thin. It carries nothing that could be mistaken for a
 * permission and nothing sensitive enough to matter if it leaked into a log —
 * no token, no entitlement, no account balance.
 */
export interface AlgorikUser {
  readonly id: string;
  readonly email: string;
  readonly emailVerified: boolean;
  readonly displayName: string | null;
  readonly accountType: AccountType;
  readonly organizationId: string | null;
  /**
   * Roles, for *rendering* decisions only — hiding a control the backend
   * would refuse anyway. Never the basis for permitting an action.
   */
  readonly roles: readonly string[];
  readonly mfaEnrolled: readonly MfaMethod[];
  readonly agreements: AccountAgreements;
}

/** A session, as the browser sees it. The credential itself stays in a cookie. */
export interface Session {
  readonly user: AlgorikUser;
  /** Epoch millis. The browser may pre-empt expiry; the backend enforces it. */
  readonly expiresAt: number;
  readonly method: AuthMethod;
  /** When the user last proved identity — drives step-up for risky actions. */
  readonly authenticatedAt: number;
}

/**
 * Every state the session can be in, including the ones that are not failures.
 *
 * `unauthenticated` and `unknown` are different: the first is a fact, the
 * second is a console that has not asked yet. Rendering a signed-out state
 * during the first paint of a signed-in session is how a user gets bounced to
 * a login page they did not need.
 */
export type SessionState =
  | { readonly status: "unknown" }
  | { readonly status: "unauthenticated" }
  | { readonly status: "authenticated"; readonly session: Session }
  | { readonly status: "expired" }
  | { readonly status: "error"; readonly reason: string };

/** A refusal, in a shape a page can render without inventing wording. */
export interface AuthFailure {
  readonly code:
    | "invalid_credentials"
    | "email_unverified"
    | "mfa_required"
    | "mfa_invalid"
    | "account_locked"
    | "agreements_required"
    | "rate_limited"
    | "provider_unavailable"
    | "session_expired"
    | "not_permitted";
  /** Shown to the user. Never contains a token, an email or an internal id. */
  readonly message: string;
  /** Where the journey should continue, when the refusal has a next step. */
  readonly next?: "verify-email" | "mfa-challenge" | "mfa-enroll" | "agreements" | "sign-in";
}

export type AuthResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly failure: AuthFailure };

export interface SignUpRequest {
  readonly email: string;
  readonly password: string;
  readonly accountType: AccountType;
  readonly displayName?: string;
  readonly agreements: AccountAgreements;
}

export interface SignInRequest {
  readonly email: string;
  readonly password: string;
}

/**
 * What every identity provider must do, and nothing more.
 *
 * The methods return `AuthResult` rather than throwing: an authentication
 * refusal is an expected answer, not an exception, and a codebase that throws
 * on a wrong password ends up with a catch block that swallows a real error.
 */
export interface IdentityProvider {
  readonly name: string;
  /** True when this provider issues its own hosted flow (e.g. OAuth redirect). */
  readonly usesRedirect: boolean;

  signUp(request: SignUpRequest): Promise<AuthResult<Session>>;
  signIn(request: SignInRequest): Promise<AuthResult<Session>>;
  /** Begin a redirect flow. Returns the URL to send the browser to. */
  startRedirect?(intendedPath: string): Promise<AuthResult<string>>;
  signOut(): Promise<void>;
  /** The current session, asked of the server. Never read from storage. */
  currentSession(): Promise<SessionState>;
  sendEmailVerification(): Promise<AuthResult<void>>;
  confirmEmailVerification(token: string): Promise<AuthResult<Session>>;
  requestPasswordReset(email: string): Promise<AuthResult<void>>;
  confirmPasswordReset(token: string, password: string): Promise<AuthResult<void>>;
  submitMfaChallenge(code: string): Promise<AuthResult<Session>>;
  /** Prove identity again, without ending the session. For risky actions. */
  reauthenticate(password: string): Promise<AuthResult<Session>>;
}

/**
 * Actions the backend must decide, and the frontend may only ask about.
 *
 * `activateLiveTrading` is deliberately absent. Algorik is paper trading as a
 * structural property, not a setting: the platform refuses a live ceiling at
 * start-up and the edge cell has no constructor that takes one. An enum member
 * for it would be the first step toward a control that expects to work, so
 * there is none. See ADR 0014.
 */
export type HighRiskAction =
  | "changeRiskLimit"
  | "changeCapitalLimit"
  | "promoteModel"
  | "deployStrategy"
  | "connectVenue"
  | "cancelAllOrders"
  | "closePosition"
  | "activateKillSwitch"
  | "recoverFromKillSwitch"
  | "walletAction";

/** How long a proof of identity stays fresh enough for a risky action. */
export const STEP_UP_WINDOW_MS = 5 * 60 * 1000;

/**
 * Whether the user must prove identity again before `action` is *attempted*.
 *
 * A convenience for the surface, never a permission: returning `false` means
 * "do not interrupt the user", not "the user may do this". The backend decides
 * that, again, and may refuse regardless.
 */
export function requiresStepUp(session: Session, now: number): boolean {
  return now - session.authenticatedAt > STEP_UP_WINDOW_MS;
}

/** Every high-risk action requires a typed reason, recorded in the audit log. */
export interface HighRiskRequest {
  readonly action: HighRiskAction;
  /** Free text the operator must supply. Stored verbatim in the audit event. */
  readonly reason: string;
  /** Present when the platform demanded a fresh proof of identity. */
  readonly reauthenticated?: boolean;
}

/** Whether a session may *see* a surface. Rendering only; never gating an action. */
export function canView(state: SessionState, roles: readonly string[] = []): boolean {
  if (state.status !== "authenticated") return false;
  if (roles.length === 0) return true;
  return roles.some((role) => state.session.user.roles.includes(role));
}

/** Where an unauthenticated visitor should land, preserving their intent. */
export function signInDestination(intendedPath: string): string {
  // Encoded, and read back through an allowlist on arrival: an open redirect
  // in a `next` parameter is the classic way a phishing link borrows a real
  // sign-in page.
  return `/sign-in?next=${encodeURIComponent(intendedPath)}`;
}

/**
 * The post-sign-in destination, refusing anything that leaves this origin.
 *
 * Only a same-origin absolute path is honoured. Anything else — a scheme, a
 * host, a protocol-relative `//evil.example` — falls back to the dashboard.
 */
export function safeRedirect(next: string | null | undefined, fallback = "/"): string {
  if (!next) return fallback;
  if (!next.startsWith("/")) return fallback;
  if (next.startsWith("//")) return fallback;
  if (next.includes("\\")) return fallback;
  return next;
}
