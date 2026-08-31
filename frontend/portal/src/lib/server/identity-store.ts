import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

/**
 * The development identity store, and nothing else (ADR 0019).
 *
 * A file-backed record of users and one-time codes for the offline
 * authentication slice — the provider that runs with no
 * `ALGORIK_IDENTITY_PROJECT_ID`, so the whole journey works and is testable
 * against no Google project at all.
 *
 * **Nothing deployed reaches this file.** It used to: the Cloud Run service
 * set `ALGORIK_IDENTITY_STORE_DIR=/tmp/algorik-identity`, where `/tmp` is
 * per-instance and in-memory, so a signed-in user was anonymous to the next
 * instance and every record vanished on scale-to-zero. Worse, the code that
 * found a record missing rebuilt it with every agreement marked accepted.
 * Identity Platform now holds the account and its custom claims hold what this
 * platform decided about it, which is one store rather than one-and-a-half.
 *
 * Sessions are no longer here at all. They are sealed claim sets in the cookie
 * itself, which any instance can verify and none has to remember.
 *
 * Deliberately not a database. One JSON file, written atomically via rename,
 * because the failure that matters in development is a half-written file after
 * a crash — rename is atomic on the same filesystem, so readers see the old
 * state or the new one, never a torn one.
 *
 * What is stored is already the production shape of *caution*: passwords only
 * as scrypt hashes, one-time codes only as HMACs, and no plaintext credential
 * anywhere. Getting the habits right in the throwaway store is the point — the
 * store is temporary, the shapes it teaches are not.
 */

export interface StoredUser {
  readonly id: string;
  readonly email: string;
  /** scrypt string from `hashPassword`; never a plaintext password. */
  readonly passwordHash: string;
  readonly displayName: string | null;
  readonly accountType: "individual" | "institutional" | "partner" | "developer";
  readonly emailVerified: boolean;
  readonly agreements: { terms: boolean; privacy: boolean; riskDisclosure: boolean };
  /** Version of the documents accepted, for re-acceptance when they change. */
  readonly agreementsVersion: number;
  readonly createdAt: number;
  readonly failedSignIns: number;
  /** Epoch ms until which sign-in is refused. 0 = not locked. */
  readonly lockedUntil: number;
  readonly roles: readonly string[];
}

export interface StoredCode {
  /** HMAC of the code, keyed like the session cookie. Never the code. */
  readonly codeHash: string;
  readonly purpose: "verify-email" | "reset-password";
  readonly userId: string;
  readonly expiresAt: number;
  readonly attemptsLeft: number;
}

interface StoreShape {
  users: Record<string, StoredUser>;
  codes: Record<string, StoredCode>;
}

const EMPTY: StoreShape = { users: {}, codes: {} };

function storePath(): string {
  const dir = process.env.ALGORIK_IDENTITY_STORE_DIR?.trim() || join(process.cwd(), ".algorik-dev");
  return join(dir, "identity.json");
}

function load(): StoreShape {
  try {
    const raw = readFileSync(storePath(), "utf8");
    const parsed = JSON.parse(raw) as StoreShape;
    return {
      users: parsed.users ?? {},
      codes: parsed.codes ?? {},
    };
  } catch {
    return { ...EMPTY, users: {}, codes: {} };
  }
}

function persist(state: StoreShape): void {
  const path = storePath();
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.tmp`;
  writeFileSync(temporary, JSON.stringify(state, null, 1));
  renameSync(temporary, path);
}

/** Every mutation goes through one read-modify-write so callers cannot tear it. */
function update<T>(mutate: (state: StoreShape) => T): T {
  const state = load();
  const result = mutate(state);
  persist(state);
  return result;
}

export const identityStore = {
  userByEmail(email: string): StoredUser | null {
    const needle = email.trim().toLowerCase();
    return Object.values(load().users).find((user) => user.email === needle) ?? null;
  },
  userById(id: string): StoredUser | null {
    return load().users[id] ?? null;
  },
  createUser(user: StoredUser): void {
    update((state) => {
      state.users[user.id] = user;
    });
  },
  patchUser(id: string, patch: Partial<StoredUser>): void {
    update((state) => {
      const existing = state.users[id];
      if (existing) state.users[id] = { ...existing, ...patch };
    });
  },

  putCode(key: string, code: StoredCode): void {
    update((state) => {
      state.codes[key] = code;
    });
  },
  code(key: string): StoredCode | null {
    const found = load().codes[key];
    if (!found) return null;
    if (found.expiresAt <= Date.now()) {
      update((state) => {
        delete state.codes[key];
      });
      return null;
    }
    return found;
  },
  /** Burn one attempt; deletes the code when none remain. */
  spendCodeAttempt(key: string): void {
    update((state) => {
      const found = state.codes[key];
      if (!found) return;
      if (found.attemptsLeft <= 1) delete state.codes[key];
      else state.codes[key] = { ...found, attemptsLeft: found.attemptsLeft - 1 };
    });
  },
  deleteCode(key: string): void {
    update((state) => {
      delete state.codes[key];
    });
  },
};
