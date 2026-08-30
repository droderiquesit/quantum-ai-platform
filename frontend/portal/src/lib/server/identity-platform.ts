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
