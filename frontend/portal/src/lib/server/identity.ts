import { createHmac, randomBytes, randomInt } from "node:crypto";
import type { AccountAgreements, AccountType, AuthFailure } from "@algorik/auth";
import { identityStore, type StoredUser } from "./identity-store";
import {
  gipReadProfile,
  gipResetPassword,
  gipSendPasswordReset,
  gipSendVerification,
  gipSignIn,
  gipSignUp,
  gipVerifyEmail,
  gipWriteProfile,
  type StoredProfileClaims,
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

/**
 * One sign-in, as the sealed cookie carries it (ADR 0019).
 *
 * Everything the console needs to answer "who is this and what may they see"
 * is here, because there is no server-side session record to look it up in.
 * That is the whole change: the previous design kept a random id in the cookie
 * and the session in a JSON file under `/tmp`, which on Cloud Run is
 * per-instance and in-memory — so a user was signed in to one instance and
 * anonymous to the next.
 *
 * `roles` is in the cookie and is therefore something the browser holds. It is
 * sealed, so a browser that edits it produces a cookie that fails the
 * signature check and reads as no session at all. What it is not is an
 * entitlement anywhere else: the platform authenticates the console by its own
 * `viewer` token, and nothing in this claim set reaches `qip-api`.
 */
export interface SessionClaims {
  readonly id: string;
  readonly userId: string;
  readonly email: string;
  readonly displayName: string | null;
  readonly accountType: AccountType;
  readonly emailVerified: boolean;
  readonly roles: readonly string[];
  readonly createdAt: number;
  readonly expiresAt: number;
  readonly authenticatedAt: number;
  readonly method: "development" | "password" | "google";
  /** Coarse device note. Never a raw user-agent dump. */
  readonly device: string;
}

/** The roles sign-up grants. Observation of the paper platform, never
 * operation of it — elevation is an operator decision with an audit trail. */
const SIGNUP_ROLES: readonly string[] = ["viewer"];

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
 * The facts the console owns about a platform account, as they are stored.
 *
 * These are custom claims on the Identity Platform account record, not a row
 * in a store of ours (ADR 0019). Google holds the credential and the mailbox
 * proof; this holds what the *platform* decided — what kind of account it is,
 * which agreements were accepted and at which version, and what the session
 * may see.
 *
 * What this replaces is worth naming, because it was a defect rather than a
 * gap. The old code kept the same fields in a JSON file under `/tmp` and, when
 * a scale event had discarded it, rebuilt the record with
 * `agreements: { terms: true, privacy: true, riskDisclosure: true }`. The
 * platform asserted a user had accepted terms it had never shown them, and the
 * assertion was manufactured from the absence of the record that would have
 * proved it.
 */
function signUpClaims(input: SignUpInput): StoredProfileClaims {
  return {
    accountType: input.accountType,
    agreements: { ...input.agreements },
    agreementsVersion: AGREEMENTS_VERSION,
    roles: [...SIGNUP_ROLES],
  };
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
      // The agreements record is written before the verification mail is
      // sent, and a failure to write it fails the sign-up. The alternative —
      // an account that exists with no record of what its owner accepted — is
      // exactly the state ADR 0019 exists to end, and it would be reached
      // silently every time this call failed.
      const stored = await gipWriteProfile(created.value.localId, signUpClaims(input));
      if (!stored) {
        return {
          ok: false,
          status: 503,
          failure: failure(
            "invalid_credentials",
            "The account was created but the agreements you accepted could not be recorded. " +
              "Use the password-reset link to finish setting the account up, or try again shortly.",
          ),
        };
      }
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
    roles: [...SIGNUP_ROLES],
  };
  identityStore.createUser(user);
  const devCode = developmentProviderActive() ? issueCode("verify-email", user.id) : null;
  return { ok: true, value: { devCode } };
}

export async function signIn(
  email: string,
  password: string,
  device: string,
): Promise<ServiceResult<SessionClaims>> {
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
    if (!signedIn.value.emailVerified) {
      return {
        ok: false,
        status: 403,
        failure: failure("email_unverified", "This email address has not been verified yet.", "verify-email"),
      };
    }

    // The agreements record decides whether this sign-in may proceed. An
    // account with no profile is an account this platform cannot show a
    // consent record for, and the answer is to ask — never to assume, and
    // never to invent one, which is what the store-backed version did every
    // time Cloud Run discarded its filesystem.
    const profile = await gipReadProfile(signedIn.value.localId);
    if (!profile) {
      return {
        ok: false,
        status: 403,
        failure: failure(
          "agreements_required",
          "This account has no record of the terms, privacy policy and risk disclosures " +
            "being accepted. Accept them to continue.",
          "agreements",
        ),
      };
    }

    const now = Date.now();
    return {
      ok: true,
      value: {
        id: newSessionId(),
        userId: `gip:${signedIn.value.localId}`,
        email: email.trim().toLowerCase(),
        displayName: signedIn.value.displayName,
        accountType: profile.accountType,
        emailVerified: true,
        roles: profile.roles,
        createdAt: now,
        expiresAt: now + SESSION_TTL_MS,
        authenticatedAt: now,
        method: "password",
        device,
      },
    };
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
  const now = Date.now();
  // The same sealed-claim session as the platform provider builds. One shape
  // in both modes, so what is exercised offline is what runs deployed.
  return {
    ok: true,
    value: {
      id: newSessionId(),
      userId: user.id,
      email: user.email,
      displayName: user.displayName,
      accountType: user.accountType,
      emailVerified: user.emailVerified,
      roles: user.roles,
      createdAt: now,
      expiresAt: now + SESSION_TTL_MS,
      authenticatedAt: now,
      method: "development",
      device,
    },
  };
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
    // Nothing local to update: `emailVerified` is Identity Platform's fact and
    // is read from the account at every sign-in. A second copy here is the
    // kind of thing that goes stale and then gets believed.
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
  // into a verification prompt. So this is a no-op there, and the response
  // shape stays identical either way.
  //
  // Returned here rather than falling through the lookup below. The
  // development store is empty in platform mode, so the lookup would reach
  // the same answer by accident, and an answer reached by accident is one
  // that changes when the accident does.
  if (!developmentProviderActive()) return { devCode: null };
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
    // Owning the mailbox settles verification, and Identity Platform records
    // that itself when it redeems the code — as it records the lockout state
    // this provider does not keep. There is nothing local left to patch.
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

/**
 * The session as the browser may see it.
 *
 * A projection rather than a lookup, now that the claims are the session. It
 * still exists, and is still the only thing a route may return, because the
 * sealed cookie holds fields the browser has no business being handed back —
 * the session handle and the device note among them. Returning the claim set
 * directly is the shortcut that turns an internal shape into a public one.
 */
export function publicSession(claims: SessionClaims): PublicSession {
  return {
    status: "authenticated",
    session: {
      user: {
        email: claims.email,
        displayName: claims.displayName,
        accountType: claims.accountType,
        emailVerified: claims.emailVerified,
        roles: claims.roles,
      },
      expiresAt: claims.expiresAt,
      authenticatedAt: claims.authenticatedAt,
    },
  };
}
