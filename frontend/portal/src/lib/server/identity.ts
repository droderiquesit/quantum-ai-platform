import { createHmac, randomBytes, randomInt } from "node:crypto";
import type { AccountAgreements, AccountType, AuthFailure } from "@algorik/auth";
import { identityStore, type StoredSession, type StoredUser } from "./identity-store";
import {
  gipResetPassword,
  gipSendPasswordReset,
  gipSendVerification,
  gipSignIn,
  gipSignUp,
  gipVerifyEmail,
} from "./identity-platform";
import { hashPassword, newSessionId, SESSION_TTL_MS, verifyPassword } from "./session";

/**
 * The identity service: every rule of the authentication journey in one
 * place, with the HTTP routes reduced to parsing and cookies.
 *
 * Decisions made here, and why:
 *
 * **Account existence is never revealed.** Sign-in returns the same failure
 * for "no such account" and "wrong password", and forgot-password returns the
 * same success either way. An endpoint that answers differently is an oracle
 * for harvesting a customer list one address at a time.
 *
 * **Lockout counts failures, not successes.** Ten failed sign-ins lock the
 * account for fifteen minutes. The count resets on success. The lock message
 * does not say the threshold or the duration — those two numbers are exactly
 * what an attacker needs to tune a slow guess to stay under.
 *
 * **One-time codes are stored as HMACs with spend-down attempts.** Five
 * guesses at a six-digit code is a 1-in-200,000 chance; unlimited guesses is
 * a certainty. The code itself exists in plaintext only in the response that
 * delivers it (development) or in the email (production).
 *
 * **Two providers, one journey.** With no ALGORIK_IDENTITY_PROJECT_ID this
 * is the development provider, self-contained and testable offline. With a
 * project configured, Google Cloud Identity Platform answers the credential
 * questions (see identity-platform.ts) and this file keeps everything that
 * is a product decision rather than a credential one: agreements, roles,
 * sessions, and what a stranger is allowed to learn. Where production behaviour must differ, the difference is marked
 * with `devCode`: a field that exists only because no email service exists
 * locally, is labelled in the UI as development-only, and is never populated
 * once a real provider is configured.
 */

const LOCKOUT_THRESHOLD = 10;
const LOCKOUT_MS = 15 * 60 * 1000;
const CODE_TTL_MS = 15 * 60 * 1000;
const CODE_ATTEMPTS = 5;
const AGREEMENTS_VERSION = 1;

function failure(code: AuthFailure["code"], message: string, next?: AuthFailure["next"]): AuthFailure {
  return next ? { code, message, next } : { code, message };
}

/** HMAC of a one-time code, so the store never holds the code itself. */
function codeHash(code: string): string {
  const key = process.env.ALGORIK_SESSION_SECRET?.trim() || "algorik-development";
  return createHmac("sha256", key).update(code).digest("base64url");
}

function newCode(): string {
  // randomInt is unbiased; Math.random would skew the first digit.
  return String(randomInt(0, 1_000_000)).padStart(6, "0");
}

function issueCode(purpose: "verify-email" | "reset-password", userId: string): string {
  const code = newCode();
  identityStore.putCode(`${purpose}:${userId}`, {
    codeHash: codeHash(code),
    purpose,
    userId,
    expiresAt: Date.now() + CODE_TTL_MS,
    attemptsLeft: CODE_ATTEMPTS,
  });
  return code;
}

function redeemCode(purpose: "verify-email" | "reset-password", userId: string, code: string): boolean {
  const key = `${purpose}:${userId}`;
  const stored = identityStore.code(key);
  if (!stored) return false;
  if (stored.codeHash !== codeHash(code.trim())) {
    identityStore.spendCodeAttempt(key);
    return false;
  }
  identityStore.deleteCode(key);
  return true;
}

/** True when the development provider is active (no Google project configured). */
export function developmentProviderActive(): boolean {
  return !process.env.ALGORIK_IDENTITY_PROJECT_ID?.trim();
}

export interface SignUpInput {
  readonly email: string;
  readonly password: string;
  readonly accountType: AccountType;
  readonly displayName?: string;
  readonly agreements: AccountAgreements;
}

export type ServiceResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly failure: AuthFailure; readonly status: number };

/**
 * Password hash sentinel for accounts whose credential lives in Identity
 * Platform. verifyPassword is never called against it in platform mode; if
 * a code path ever does, no password hashes to this string.
 */
const EXTERNAL_CREDENTIAL = "external:identity-platform";

/** The local record for a platform-authenticated user: everything that is
 * ours to decide (roles, agreements, account type) and nothing that is
 * Google's (the password). Created at sign-up, or on first sign-in for an
 * account that predates this store. */
function platformProfile(
  email: string,
  localId: string,
  displayName: string | null,
  input?: SignUpInput,
): StoredUser {
  const existing = identityStore.userByEmail(email);
  if (existing) return existing;
  const user: StoredUser = {
    id: `gip:${localId}`,
    email,
    passwordHash: EXTERNAL_CREDENTIAL,
    displayName: input?.displayName?.trim() || displayName,
    accountType: input?.accountType ?? "individual",
    emailVerified: false,
    agreements: input ? { ...input.agreements } : { terms: true, privacy: true, riskDisclosure: true },
    agreementsVersion: AGREEMENTS_VERSION,
    createdAt: Date.now(),
    failedSignIns: 0,
    lockedUntil: 0,
    // Viewer only, in both providers. Sign-up grants observation of the
    // paper platform, never operation of it.
    roles: ["viewer"],
  };
  identityStore.createUser(user);
  return user;
}

export async function signUp(input: SignUpInput): Promise<ServiceResult<{ devCode: string | null }>> {
  const email = input.email.trim().toLowerCase();
  if (!input.agreements.terms || !input.agreements.privacy || !input.agreements.riskDisclosure) {
    return {
      ok: false,
      status: 400,
      failure: failure("agreements_required", "The terms, privacy policy and risk disclosures must each be accepted.", "agreements"),
    };
  }

  if (!developmentProviderActive()) {
    const created = await gipSignUp(email, input.password);
    if (created.ok) {
      platformProfile(email, created.value.localId, null, input);
      await gipSendVerification(created.value.idToken);
    } else if (created.error.code !== "EMAIL_EXISTS") {
      // EMAIL_EXISTS falls through to the same shape as success — account
      // existence is never revealed. Anything else is a real refusal.
      const message =
        created.error.code === "WEAK_PASSWORD"
          ? "That password is too short for this platform. Use at least six characters."
          : "Sign-up was not accepted. Check the details and try again.";
      return { ok: false, status: 400, failure: failure("invalid_credentials", message) };
    }
    return { ok: true, value: { devCode: null } };
  }

  const existing = identityStore.userByEmail(email);
  if (existing) {
    // Same shape as success: an attacker learns nothing, and the person who
    // genuinely forgot they registered gets a working next step, because the
    // verification path doubles as "prove you own this mailbox".
    const devCode = developmentProviderActive() && !existing.emailVerified
      ? issueCode("verify-email", existing.id)
      : null;
    return { ok: true, value: { devCode } };
  }

  const user: StoredUser = {
    id: randomBytes(16).toString("base64url"),
    email,
    passwordHash: await hashPassword(input.password),
    displayName: input.displayName?.trim() || null,
    accountType: input.accountType,
    emailVerified: false,
    agreements: { ...input.agreements },
    agreementsVersion: AGREEMENTS_VERSION,
    createdAt: Date.now(),
    failedSignIns: 0,
    lockedUntil: 0,
    // Viewer only. Sign-up grants observation of the paper platform, never
    // operation of it — elevation is an operator decision with an audit trail.
    roles: ["viewer"],
  };
  identityStore.createUser(user);
  const devCode = developmentProviderActive() ? issueCode("verify-email", user.id) : null;
  return { ok: true, value: { devCode } };
}

export async function signIn(
  email: string,
  password: string,
  device: string,
): Promise<ServiceResult<{ session: StoredSession; user: StoredUser }>> {
  if (!developmentProviderActive()) {
    const signedIn = await gipSignIn(email.trim().toLowerCase(), password);
    if (!signedIn.ok) {
      // Google answers INVALID_LOGIN_CREDENTIALS for missing account and
      // wrong password alike, which is exactly the discipline the local
      // provider implements with a decoy hash.
      return {
        ok: false,
        status: 401,
        failure: failure("invalid_credentials", "That email and password combination was not accepted."),
      };
    }
    const profile = platformProfile(email.trim().toLowerCase(), signedIn.value.localId, signedIn.value.displayName);
    if (!signedIn.value.emailVerified) {
      return {
        ok: false,
        status: 403,
        failure: failure("email_unverified", "This email address has not been verified yet.", "verify-email"),
      };
    }
    if (profile.emailVerified !== true) identityStore.patchUser(profile.id, { emailVerified: true });
    const session: StoredSession = {
      id: newSessionId(),
      userId: profile.id,
      createdAt: Date.now(),
      expiresAt: Date.now() + SESSION_TTL_MS,
      authenticatedAt: Date.now(),
      method: "password",
      device,
    };
    identityStore.createSession(session);
    return { ok: true, value: { session, user: { ...profile, emailVerified: true } } };
  }

  const user = identityStore.userByEmail(email);
  // The password is verified even when the user is missing, against a real
  // hash of a random value, so the response time does not say which branch
  // ran. A fast "no such user" is a timing oracle for account existence.
  const decoyHash = await hashPassword(randomBytes(8).toString("hex"));
  const passwordOk = await verifyPassword(password, user?.passwordHash ?? decoyHash);

  if (!user || !passwordOk) {
    if (user) {
      const failed = user.failedSignIns + 1;
      identityStore.patchUser(user.id, {
        failedSignIns: failed,
        lockedUntil: failed >= LOCKOUT_THRESHOLD ? Date.now() + LOCKOUT_MS : user.lockedUntil,
      });
    }
    return {
      ok: false,
      status: 401,
      failure: failure("invalid_credentials", "That email and password combination was not accepted."),
    };
  }

  if (user.lockedUntil > Date.now()) {
    return {
      ok: false,
      status: 423,
      failure: failure("account_locked", "This account is temporarily locked after repeated failed sign-ins. Try again later, or reset the password."),
    };
  }

  if (!user.emailVerified) {
    return {
      ok: false,
      status: 403,
      failure: failure("email_unverified", "This email address has not been verified yet.", "verify-email"),
    };
  }

  identityStore.patchUser(user.id, { failedSignIns: 0, lockedUntil: 0 });
  const session: StoredSession = {
    id: newSessionId(),
    userId: user.id,
    createdAt: Date.now(),
    expiresAt: Date.now() + SESSION_TTL_MS,
    authenticatedAt: Date.now(),
    method: developmentProviderActive() ? "development" : "password",
    device,
  };
  identityStore.createSession(session);
  return { ok: true, value: { session, user } };
}

export async function verifyEmail(email: string, code: string): Promise<ServiceResult<null>> {
  if (!developmentProviderActive()) {
    const redeemed = await gipVerifyEmail(code);
    if (!redeemed.ok) {
      return {
        ok: false,
        status: 400,
        failure: failure("invalid_credentials", "That code was not accepted. Request a new verification email by signing in again."),
      };
    }
    const profile = identityStore.userByEmail(email.trim().toLowerCase());
    if (profile) identityStore.patchUser(profile.id, { emailVerified: true });
    return { ok: true, value: null };
  }

  const user = identityStore.userByEmail(email);
  if (!user || !redeemCode("verify-email", user.id, code)) {
    return {
      ok: false,
      status: 400,
      failure: failure("invalid_credentials", "That code was not accepted. Codes expire after fifteen minutes — request a new one if needed."),
    };
  }
  identityStore.patchUser(user.id, { emailVerified: true });
  return { ok: true, value: null };
}

export function resendVerification(email: string): { devCode: string | null } {
  // In platform mode re-sending needs a fresh idToken, and the way to get
  // one is signing in — which the unverified-sign-in path already turns
  // into a verification prompt. So this stays a no-op there, and the
  // response shape stays identical either way.
  const user = identityStore.userByEmail(email);
  if (!user || user.emailVerified) return { devCode: null };
  return { devCode: developmentProviderActive() ? issueCode("verify-email", user.id) : null };
}

export async function forgotPassword(email: string): Promise<{ devCode: string | null }> {
  if (!developmentProviderActive()) {
    // Google sends the mail; EMAIL_NOT_FOUND is swallowed inside the
    // provider so this answer never says which addresses exist.
    await gipSendPasswordReset(email.trim().toLowerCase());
    return { devCode: null };
  }
  const user = identityStore.userByEmail(email);
  if (!user) return { devCode: null };
  return { devCode: developmentProviderActive() ? issueCode("reset-password", user.id) : null };
}

export async function resetPassword(email: string, code: string, password: string): Promise<ServiceResult<null>> {
  if (!developmentProviderActive()) {
    const reset = await gipResetPassword(code, password);
    if (!reset.ok) {
      return {
        ok: false,
        status: 400,
        failure: failure("invalid_credentials", "That code was not accepted. Codes expire — request a new reset email if needed."),
      };
    }
    const profile = identityStore.userByEmail(email.trim().toLowerCase());
    // Owning the mailbox settles verification here exactly as it does in
    // the development provider.
    if (profile) identityStore.patchUser(profile.id, { emailVerified: true, failedSignIns: 0, lockedUntil: 0 });
    return { ok: true, value: null };
  }

  const user = identityStore.userByEmail(email);
  if (!user || !redeemCode("reset-password", user.id, code)) {
    return {
      ok: false,
      status: 400,
      failure: failure("invalid_credentials", "That code was not accepted. Codes expire after fifteen minutes — request a new one if needed."),
    };
  }
  identityStore.patchUser(user.id, {
    passwordHash: await hashPassword(password),
    failedSignIns: 0,
    lockedUntil: 0,
    // Owning the mailbox is the strongest proof this flow sees; it also
    // settles verification for an account that never finished signing up.
    emailVerified: true,
  });
  return { ok: true, value: null };
}

export interface PublicSession {
  readonly status: "authenticated";
  readonly session: {
    readonly user: {
      readonly email: string;
      readonly displayName: string | null;
      readonly accountType: AccountType;
      readonly emailVerified: boolean;
      readonly roles: readonly string[];
    };
    readonly expiresAt: number;
    readonly authenticatedAt: number;
  };
}

/** The session as the browser may see it. No id, no hash, no entitlement. */
export function publicSession(sessionId: string): PublicSession | null {
  const session = identityStore.session(sessionId);
  if (!session) return null;
  const user = identityStore.userById(session.userId);
  if (!user) return null;
  return {
    status: "authenticated",
    session: {
      user: {
        email: user.email,
        displayName: user.displayName,
        accountType: user.accountType,
        emailVerified: user.emailVerified,
        roles: user.roles,
      },
      expiresAt: session.expiresAt,
      authenticatedAt: session.authenticatedAt,
    },
  };
}
