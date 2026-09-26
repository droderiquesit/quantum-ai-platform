/**
 * The key every email-verification and password-reset code is hashed under.
 *
 * No browser and no server: these tests import `src/lib/server/identity.ts`
 * and drive its own exports, `signUp` and `forgotPassword`, then read the hash
 * they stored from the development store's `identity.json`. That is on
 * purpose. The defect lived in `identity.ts` — its `codeHash` keyed codes with
 * the configured session secret *or else a string literal*, while the
 * production refusal lived in `session.ts`'s `signingKey`, which that path
 * never called — so a test of a `session.ts` export alone stays green when the
 * literal comes back in `identity.ts`, which is the regression that matters.
 *
 * What the key protects, and no more: the development provider stores each
 * code only as its HMAC, and under a key printed in this repository anyone
 * who can read the store recovers a six-digit code in at most a million HMACs.
 * It is **not** what protects the reset flow on a production build of the
 * development provider, which returns the plaintext code in the HTTP
 * response. That exposure is a separate defect with a separate owner, and
 * nothing here claims to close it.
 *
 * Every test runs with no identity project, so `developmentProviderActive()`
 * is true and the branch that hashes codes is the one answering; each points
 * `ALGORIK_IDENTITY_STORE_DIR` at a fresh temporary directory, so the store it
 * reads is its own. The environment is restored after each test, because
 * Playwright runs several tests in one worker process and a `NODE_ENV` left
 * as `production` would change what the next one proves.
 */
import { createHmac } from "node:crypto";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import {
  developmentProviderActive,
  forgotPassword,
  signUp,
  verifyEmail,
  type SignUpInput,
} from "../src/lib/server/identity";
import { sealClaims } from "../src/lib/server/session";

/** Every variable a test here sets or clears, restored exactly afterwards. */
const TOUCHED = [
  "NODE_ENV",
  "ALGORIK_SESSION_SECRET",
  "ALGORIK_SESSION_SECRET_FILE",
  "ALGORIK_IDENTITY_PROJECT_ID",
  "ALGORIK_IDENTITY_STORE_DIR",
] as const;

type Touched = (typeof TOUCHED)[number];

/**
 * The fallback the defect keyed codes with, written here and nowhere under
 * `src/`. It is not a secret: it was published in the source, which is the
 * whole of what was wrong with it.
 */
const DEFECT_LITERAL = "algorik-development";

/** A configured value long enough to pass the length check, visibly a test value. */
const LONG_ENOUGH = "one-time-code-key-spec-configured-value-00";

/** Sixteen characters: half the minimum, and visibly a test value. */
const TOO_SHORT = "too-short-for-it";

const PASSWORD = "a-long-development-passphrase";

/** Delimited, because the first name is a prefix of the second. */
const NAMES_THE_VARIABLE = /\bALGORIK_SESSION_SECRET\b/u;
const NAMES_THE_FILE_VARIABLE = /\bALGORIK_SESSION_SECRET_FILE\b/u;

/** What `signUp` hashes to: 32 bytes of HMAC-SHA256, unpadded base64url. */
const HMAC_SHA256_BASE64URL = /^[A-Za-z0-9_-]{43}$/u;

interface StoreFile {
  readonly users: Record<string, { readonly id: string; readonly email: string }>;
  readonly codes: Record<
    string,
    { readonly codeHash: string; readonly purpose: string; readonly userId: string }
  >;
}

let saved: Partial<Record<Touched, string>> = {};
let storeDir = "";

function setVariable(name: Touched, value: string | undefined): void {
  // NODE_ENV is typed readonly under Next; Reflect writes it without a cast
  // that the compiler or the linter would refuse.
  if (value === undefined) Reflect.deleteProperty(process.env, name);
  else Reflect.set(process.env, name, value);
}

function setEnvironment(values: Partial<Record<Touched, string | undefined>>): void {
  for (const name of TOUCHED) {
    if (name in values) setVariable(name, values[name]);
  }
}

test.beforeEach(() => {
  saved = {};
  for (const name of TOUCHED) {
    const value = process.env[name];
    if (value !== undefined) saved[name] = value;
  }
  storeDir = mkdtempSync(join(tmpdir(), "algorik-one-time-code-"));
  setEnvironment({
    ALGORIK_IDENTITY_STORE_DIR: storeDir,
    ALGORIK_IDENTITY_PROJECT_ID: undefined,
    ALGORIK_SESSION_SECRET: undefined,
    ALGORIK_SESSION_SECRET_FILE: undefined,
  });
});

test.afterEach(() => {
  for (const name of TOUCHED) setVariable(name, saved[name]);
  rmSync(storeDir, { recursive: true, force: true });
});

function account(email: string): SignUpInput {
  return {
    email,
    password: PASSWORD,
    accountType: "individual",
    agreements: { terms: true, privacy: true, riskDisclosure: true },
  };
}

function readStore(): StoreFile {
  const path = join(storeDir, "identity.json");
  if (!existsSync(path)) return { users: {}, codes: {} };
  return JSON.parse(readFileSync(path, "utf8")) as StoreFile;
}

/** The error a call was refused with, or null if it completed. */
async function refusalOf(pending: Promise<unknown>): Promise<Error | null> {
  try {
    await pending;
    return null;
  } catch (error) {
    return error instanceof Error ? error : new Error(String(error));
  }
}

function refusalOfSync(call: () => unknown): Error | null {
  try {
    call();
    return null;
  } catch (error) {
    return error instanceof Error ? error : new Error(String(error));
  }
}

/**
 * Every string literal written in the server library, as a candidate key.
 *
 * The test below is named for *a string written in the source*, not for one
 * string, so it tries them all: restoring the old fallback is the regression
 * named, and swapping it for a different literal is the same defect spelled
 * differently. Scanned rather than listed, so a literal added tomorrow is a
 * candidate tomorrow. Template literals with a substitution are skipped; they
 * are not a fixed string.
 *
 * Each quote style is scanned on its own and within one line. A single
 * combined pattern let a backtick in a doc comment open a span that ran across
 * lines and swallowed the real literals inside it — the premise below caught
 * exactly that on the first run, which is why it is there. Scanning too much
 * costs only a few extra candidate keys; scanning too little would let a
 * literal key through unchecked.
 */
function literalsInServerSource(): Set<string> {
  const directory = join(__dirname, "..", "src", "lib", "server");
  const styles = [/"((?:[^"\\\n]|\\.)*)"/gu, /'((?:[^'\\\n]|\\.)*)'/gu, /`((?:[^`\\$\n]|\\.)*)`/gu];
  const found = new Set<string>();
  for (const file of readdirSync(directory)) {
    if (!file.endsWith(".ts")) continue;
    const source = readFileSync(join(directory, file), "utf8");
    for (const style of styles) {
      for (const match of source.matchAll(style)) {
        if (match[1] !== undefined) found.add(match[1]);
      }
    }
  }
  return found;
}

test("a one-time code is refused in production when no session secret is configured", async () => {
  // An account that exists before the secret goes missing, created the way a
  // healthy deployment would create it, so forgot-password has somebody to
  // issue a code for.
  const existing = "existing-account@algorik.test";
  setEnvironment({ NODE_ENV: "production", ALGORIK_SESSION_SECRET: LONG_ENOUGH });
  expect(
    developmentProviderActive(),
    "an identity project is configured, so the branch that hashes codes is not the one under test",
  ).toBe(true);
  const created = await signUp(account(existing));
  expect(created.ok, "the account forgot-password needs could not be created under a configured secret").toBe(true);
  expect(Object.values(readStore().users).map((user) => user.email)).toContain(existing);

  // The deployment whose secret file failed to mount: production, and neither
  // half of the `_FILE` contract set.
  setEnvironment({ ALGORIK_SESSION_SECRET: undefined, ALGORIK_SESSION_SECRET_FILE: undefined });
  expect(process.env.NODE_ENV).toBe("production");
  expect(process.env.ALGORIK_SESSION_SECRET).toBeUndefined();
  expect(process.env.ALGORIK_SESSION_SECRET_FILE).toBeUndefined();
  expect(developmentProviderActive()).toBe(true);
  const codesBefore = readStore().codes;

  const signUpRefusal = await refusalOf(signUp(account("new-account@algorik.test")));
  expect(signUpRefusal, "sign-up issued a verification code with no session secret in production").not.toBeNull();
  expect(signUpRefusal?.message).toMatch(NAMES_THE_VARIABLE);
  expect(signUpRefusal?.message).toMatch(NAMES_THE_FILE_VARIABLE);

  const resetRefusal = await refusalOf(forgotPassword(existing));
  expect(resetRefusal, "forgot-password issued a reset code with no session secret in production").not.toBeNull();
  expect(resetRefusal?.message).toMatch(NAMES_THE_VARIABLE);
  expect(resetRefusal?.message).toMatch(NAMES_THE_FILE_VARIABLE);

  // Neither refusal left a code behind: the store holds exactly what it held
  // before, which is the verification code the healthy sign-up issued.
  expect(readStore().codes).toEqual(codesBefore);
});

test("a one-time code in development is never keyed by a string written in the source", async () => {
  const email = "development-account@algorik.test";
  setEnvironment({ NODE_ENV: "development" });
  expect(developmentProviderActive()).toBe(true);

  const created = await signUp(account(email));
  expect(created.ok).toBe(true);
  const devCode = created.ok ? created.value.devCode : null;
  expect(devCode, "the development provider returned no code, so there is no hash to judge").toMatch(/^\d{6}$/u);

  const store = readStore();
  const user = Object.values(store.users).find((candidate) => candidate.email === email);
  expect(user, "sign-up wrote no account").toBeDefined();
  const records = Object.values(store.codes).filter(
    (record) => record.purpose === "verify-email" && record.userId === user?.id,
  );
  expect(records).toHaveLength(1);
  const stored = records[0]?.codeHash ?? "";
  expect(stored).toMatch(HMAC_SHA256_BASE64URL);

  // Premise for the sweep: the scanner reads the server source — `session.ts`
  // names its hash with the first literal, and `identity.ts`, the defect
  // site, names a code purpose with the second — and the named fallback is a
  // candidate whether or not it is still in the source.
  const candidates = literalsInServerSource();
  expect(candidates.has("sha256"), "the literal scan did not read session.ts's literals").toBe(true);
  expect(candidates.has("verify-email"), "the literal scan did not read identity.ts's literals").toBe(true);
  candidates.add(DEFECT_LITERAL);

  const underLiteral = createHmac("sha256", DEFECT_LITERAL).update(devCode ?? "").digest("base64url");
  expect(stored, "the code was hashed under the development literal").not.toBe(underLiteral);
  const keyedBy = [...candidates].find(
    (key) => createHmac("sha256", key).update(devCode ?? "").digest("base64url") === stored,
  );
  expect(keyedBy, "the code was hashed under a string written in the source").toBeUndefined();

  // And the hash is a real one: the code redeems against it in this process,
  // so what was stored is the HMAC of this code under the key this process
  // actually holds, not bytes that merely differ from the literal's.
  const redeemed = await verifyEmail(email, devCode ?? "");
  expect(redeemed.ok).toBe(true);
});

test("a configured secret shorter than a signing key is refused for codes exactly as it is for cookies", async () => {
  setEnvironment({ NODE_ENV: "development", ALGORIK_SESSION_SECRET: TOO_SHORT });
  expect(TOO_SHORT).toHaveLength(16);
  expect(developmentProviderActive()).toBe(true);

  // The cookie signer's refusal is what codes must match, so it has to exist.
  const cookieRefusal = refusalOfSync(() => sealClaims({ premise: "a session" }));
  expect(cookieRefusal, "the cookie signer accepted a 16-character key, so there is nothing to match").not.toBeNull();

  const codeRefusal = await refusalOf(signUp(account("short-key-account@algorik.test")));
  expect(codeRefusal, "sign-up issued a code under a 16-character key").not.toBeNull();
  expect(codeRefusal?.message).toMatch(/\bshorter than 32 characters\b/u);
  expect(codeRefusal?.message).toBe(cookieRefusal?.message);
  expect(codeRefusal?.message, "the refusal printed the value it refused").not.toContain(TOO_SHORT);
  expect(readStore().codes).toEqual({});
});
