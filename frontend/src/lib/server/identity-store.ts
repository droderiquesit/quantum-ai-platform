import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

/**
 * The development identity store.
 *
 * A file-backed record of users, sessions and one-time codes, for the local
 * authentication slice. It exists so the entire journey — sign-up,
 * verification, session, sign-out, reset — works and is testable before any
 * Google project does, and its interface is the contract a production store
 * (Identity Platform plus the platform's own user record) implements later.
 *
 * Deliberately not a database. One JSON file, written atomically via rename,
 * because the failure that matters in development is a half-written file after
 * a crash — rename is atomic on the same filesystem, so readers see the old
 * state or the new one, never a torn one.
 *
 * What is stored is already the production shape of *caution*: passwords only
 * as scrypt hashes, one-time codes only as HMACs, session ids in the clear
 * (they are random handles, not secrets derived from anything), and no
 * plaintext credential anywhere. Getting the habits right in the throwaway
 * store is the point — the store is temporary, the shapes it teaches are not.
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

export interface StoredSession {
  readonly id: string;
  readonly userId: string;
  readonly createdAt: number;
  readonly expiresAt: number;
  readonly authenticatedAt: number;
  readonly method: "development" | "password" | "google";
  /** Coarse device note for the session list; never a raw user-agent dump. */
  readonly device: string;
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
  sessions: Record<string, StoredSession>;
  codes: Record<string, StoredCode>;
}

const EMPTY: StoreShape = { users: {}, sessions: {}, codes: {} };

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
      sessions: parsed.sessions ?? {},
      codes: parsed.codes ?? {},
    };
  } catch {
    return { ...EMPTY, users: {}, sessions: {}, codes: {} };
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

  createSession(session: StoredSession): void {
    update((state) => {
      state.sessions[session.id] = session;
    });
  },
  session(id: string): StoredSession | null {
    const found = load().sessions[id];
    if (!found) return null;
    if (found.expiresAt <= Date.now()) {
      // Expiry is enforced at read: a session that outlived its clock is
      // removed here rather than trusted until a sweeper runs.
      update((state) => {
        delete state.sessions[id];
      });
      return null;
    }
    return found;
  },
  deleteSession(id: string): void {
    update((state) => {
      delete state.sessions[id];
    });
  },
  sessionsForUser(userId: string): readonly StoredSession[] {
    return Object.values(load().sessions)
      .filter((session) => session.userId === userId && session.expiresAt > Date.now())
      .sort((a, b) => b.createdAt - a.createdAt);
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
