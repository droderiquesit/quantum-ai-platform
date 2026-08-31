/**
 * The Google Cloud Identity Platform provider — the production half of the
 * seam `developmentProviderActive()` switches on.
 *
 * Everything here runs server-side in the BFF. The browser never talks to
 * Google directly and never sees an idToken: Google answers the credential
 * question, and the session the browser holds stays the BFF's own signed
 * cookie, so the gateway, CSRF and session machinery are identical in both
 * modes. Google also does its own attempt-limiting, which is why the local
 * lockout counters are not consulted in this mode.
 *
 * The API key is the Identity Platform browser key: public by design and
 * restricted by the authorized-domains list Terraform manages. It selects a
 * project; it authenticates nobody.
 */

import { accessToken } from "./google-credentials";

const ENDPOINT = "https://identitytoolkit.googleapis.com/v1";
/** Every call that leaves the process carries an explicit timeout. */
const TIMEOUT_MS = 10_000;

function apiKey(): string {
  const key = process.env.ALGORIK_IDENTITY_API_KEY?.trim();
  if (!key) {
    // Fail closed and loudly: a configured project with no key is a broken
    // deployment, not a reason to fall back to the development store.
    throw new Error(
      "ALGORIK_IDENTITY_PROJECT_ID is set but ALGORIK_IDENTITY_API_KEY is not; " +
        "customer identity cannot run half-configured.",
    );
  }
  return key;
}

interface GipError {
  readonly code: string;
}

type GipResult<T> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly error: GipError };

async function call<T>(method: string, body: Record<string, unknown>): Promise<GipResult<T>> {
  const response = await fetch(`${ENDPOINT}/${method}?key=${apiKey()}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  const payload = (await response.json().catch(() => ({}))) as {
    error?: { message?: string };
  } & T;
  if (!response.ok) {
    // Google's message field carries a stable ALL_CAPS code, sometimes with
    // a trailing explanation after " : ". The code is the contract.
    const code = (payload.error?.message ?? "UNKNOWN").split(" ")[0] ?? "UNKNOWN";
    return { ok: false, error: { code } };
  }
  return { ok: true, value: payload };
}

export interface GipAccount {
  readonly localId: string;
  readonly idToken: string;
}

/**
 * Create the account. EMAIL_EXISTS is reported as such so the caller can
 * keep its indistinguishable-response discipline — the decision of what a
 * stranger learns belongs in one place, and it is not here.
 */
export async function gipSignUp(email: string, password: string): Promise<GipResult<GipAccount>> {
  return call<GipAccount>("accounts:signUp", { email, password, returnSecureToken: true });
}

/** Ask Google to send the verification mail for a freshly issued idToken. */
export async function gipSendVerification(idToken: string): Promise<void> {
  // Best-effort by design: the account exists either way, and the resend
  // path is signing in again, which re-issues a token.
  await call("accounts:sendOobCode", { requestType: "VERIFY_EMAIL", idToken }).catch(() => undefined);
}

export interface GipSignIn {
  readonly localId: string;
  readonly idToken: string;
  readonly emailVerified: boolean;
  readonly displayName: string | null;
}

export async function gipSignIn(email: string, password: string): Promise<GipResult<GipSignIn>> {
  const signedIn = await call<GipAccount>("accounts:signInWithPassword", {
    email,
    password,
    returnSecureToken: true,
  });
  if (!signedIn.ok) return signedIn;
  // emailVerified lives on the account record, not the sign-in response.
  const looked = await call<{ users?: ReadonlyArray<{ emailVerified?: boolean; displayName?: string }> }>(
    "accounts:lookup",
    { idToken: signedIn.value.idToken },
  );
  const record = looked.ok ? looked.value.users?.[0] : undefined;
  return {
    ok: true,
    value: {
      localId: signedIn.value.localId,
      idToken: signedIn.value.idToken,
      emailVerified: record?.emailVerified === true,
      displayName: record?.displayName?.trim() || null,
    },
  };
}

/** Trigger the password-reset mail. EMAIL_NOT_FOUND is deliberately not an
 * error to the caller: the response must not say which addresses exist. */
export async function gipSendPasswordReset(email: string): Promise<void> {
  await call("accounts:sendOobCode", { requestType: "PASSWORD_RESET", email }).catch(() => undefined);
}

/** Redeem the emailed oobCode for a new password. */
export async function gipResetPassword(oobCode: string, newPassword: string): Promise<GipResult<unknown>> {
  return call("accounts:resetPassword", { oobCode, newPassword });
}

/** Redeem the emailed oobCode that proves mailbox ownership. */
export async function gipVerifyEmail(oobCode: string): Promise<GipResult<unknown>> {
  return call("accounts:update", { oobCode });
}

// ---------------------------------------------------------------------------
// The administrative half: the facts the console owns, on the account record
// ---------------------------------------------------------------------------

/**
 * Everything above this line uses the browser API key, which selects a project
 * and authenticates nobody. Everything below authenticates the console as
 * itself, through the metadata server, and reaches the project-scoped
 * endpoints the key cannot.
 *
 * Why here at all (ADR 0019): account type, the agreements accepted and their
 * version, and roles are decisions this platform makes, not credentials Google
 * holds. They were kept in a JSON file under `/tmp` on Cloud Run, which is
 * per-instance and in-memory, so they did not survive a scale event — and the
 * code that found them missing invented them, recording that a user had
 * accepted terms nobody had shown them. Custom claims put them where the
 * account is, so there is one record and nothing to reconcile.
 *
 * The claim payload is capped by Google at 1000 bytes. What is stored here is
 * four small fields and stays far inside that; `PROFILE_CLAIM_LIMIT` asserts
 * it rather than trusting it, because the failure at the boundary is Google
 * refusing the whole update and the console then behaving as though the user
 * had no profile.
 */

const ADMIN_ENDPOINT = "https://identitytoolkit.googleapis.com/v1/projects";

/** Google's documented ceiling for the serialised custom-claims blob. */
const PROFILE_CLAIM_LIMIT = 1000;

/** The claim key. Namespaced, so it cannot collide with a reserved name. */
const PROFILE_CLAIM = "algorik";

export interface StoredProfileClaims {
  readonly accountType: "individual" | "institutional" | "partner" | "developer";
  readonly agreements: { terms: boolean; privacy: boolean; riskDisclosure: boolean };
  readonly agreementsVersion: number;
  readonly roles: readonly string[];
}

function projectId(): string {
  const project = process.env.ALGORIK_IDENTITY_PROJECT_ID?.trim();
  if (!project) {
    throw new Error(
      "ALGORIK_IDENTITY_PROJECT_ID is not set, so there is no project whose accounts " +
        "this could administer. The development provider does not reach this path.",
    );
  }
  return project;
}

async function adminCall<T>(method: string, body: Record<string, unknown>): Promise<T | null> {
  const token = await accessToken();
  const response = await fetch(`${ADMIN_ENDPOINT}/${projectId()}/${method}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  if (!response.ok) return null;
  return (await response.json().catch(() => null)) as T | null;
}

/**
 * Read the console's own facts about an account.
 *
 * `null` means "this account carries no profile", which is a different answer
 * from "the account has no agreements" and is treated differently by the
 * caller: the first sends the user to re-accept, the second is impossible
 * because an accepted set is the only thing ever written.
 */
export async function gipReadProfile(localId: string): Promise<StoredProfileClaims | null> {
  const looked = await adminCall<{
    users?: ReadonlyArray<{ customAttributes?: string }>;
  }>("accounts:lookup", { localId: [localId] });

  const raw = looked?.users?.[0]?.customAttributes;
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const profile = parsed[PROFILE_CLAIM] as StoredProfileClaims | undefined;
    // A claim blob written by something else, or by an older shape, must read
    // as absent rather than as a partially populated profile.
    if (!profile || typeof profile !== "object") return null;
    if (!profile.agreements || typeof profile.agreementsVersion !== "number") return null;
    if (!Array.isArray(profile.roles)) return null;
    return profile;
  } catch {
    return null;
  }
}

/**
 * Write the console's own facts onto an account.
 *
 * Returns whether it was stored. A false answer is not swallowed by the
 * caller: a sign-up whose agreements were not recorded has no record that the
 * user agreed, and the honest thing is to say the sign-up did not complete
 * rather than to leave an account behind that will be asked to re-accept
 * without knowing why.
 */
export async function gipWriteProfile(
  localId: string,
  profile: StoredProfileClaims,
): Promise<boolean> {
  const serialised = JSON.stringify({ [PROFILE_CLAIM]: profile });
  if (serialised.length > PROFILE_CLAIM_LIMIT) {
    // Refuse locally rather than let Google refuse the whole update: its
    // error would arrive as a generic failure and the account would be left
    // with no profile at all.
    return false;
  }
  const written = await adminCall<unknown>("accounts:update", {
    localId,
    customAttributes: serialised,
  });
  return written !== null;
}
