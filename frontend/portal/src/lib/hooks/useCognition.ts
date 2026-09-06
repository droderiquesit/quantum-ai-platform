"use client";

import { request } from "@/lib/api/client";
import { useResource, type Resource } from "./useResource";

/**
 * The cognition plane, read-only: two viewer routes and the shapes they answer.
 *
 * `GET /cognition/self-model` is the platform's own account of how accurate
 * each of its origins — detector, analyst, rung, strategy family — has
 * measured itself to be from a bounded window of graded outcomes, and
 * `GET /cognition/precedents` is what its episodic memory last recalled. This
 * console reads them and renders them. It grades nothing, computes no
 * accuracy, shrinks no estimate and recalls no episode: every figure on the
 * cognition pages is a field one of these routes answered, and a row the
 * platform refused to estimate is rendered as refused rather than as zero.
 *
 * Nothing in this file is a write. There is no fetcher here that grades an
 * outcome, re-weights an origin or stores an episode, and `client.ts`
 * declares none: the gateway refuses any non-GET this console does not
 * declare before the credential is read (`tests/boundary.spec.ts`).
 *
 * Shapes are transcribed from the route contract as it was stated to this
 * console: snake_case keys, `accuracy` a decimal string or `null` below the
 * minimum sample, and a precedent an object whose fields the memory chooses.
 */

const POLL_MS = 15_000;

// --- GET /cognition/self-model ---------------------------------------------------

/** A `qip_core::Decimal`, serialised as its exact decimal string. Never parsed here. */
export type DecimalString = string;

/**
 * One `CapabilityEstimate`: an origin's measured accuracy over its graded
 * window. `accuracy` is `null` where `samples` is below `minimum_sample`,
 * because the platform refuses an estimate it has too few outcomes to hold —
 * a `null` here is a refusal, not a zero.
 */
export interface SelfModelComponent {
  /** The class of origin: detector, analyst, rung or strategy family. */
  readonly kind: string;
  /** Which one, within its kind. */
  readonly key: string;
  readonly samples: number;
  readonly accuracy: DecimalString | null;
  /** Whether the sample met the minimum and an estimate exists. */
  readonly calibrated: boolean;
}

export interface SelfModel {
  /** In the platform's own order; rendered as received. */
  readonly components: readonly SelfModelComponent[];
  /** The sample below which the platform refuses an estimate. */
  readonly minimum_sample: number;
}

// --- GET /cognition/precedents -----------------------------------------------------

/**
 * One recalled episode. The memory chooses the fields; the page renders
 * whichever are present, `similarity`, `outcome` and `age` first when they
 * are, and the rest in the order they came.
 */
export type Precedent = Readonly<Record<string, unknown>>;

export interface Precedents {
  readonly precedents: readonly Precedent[];
}

// --- fetchers ----------------------------------------------------------------------

/**
 * The two reads, and the only two. Kept here rather than in `platform` so a
 * reviewer of the cognition surface sees every request it can make in one
 * place, and sees that each is a GET.
 */
export const cognition = {
  selfModel: (signal?: AbortSignal) => request<SelfModel>("/cognition/self-model", signal ? { signal } : {}),
  precedents: (signal?: AbortSignal) => request<Precedents>("/cognition/precedents", signal ? { signal } : {}),
} as const;

export function useSelfModel(): Resource<SelfModel> {
  return useResource<SelfModel>(cognition.selfModel, {
    key: "cognition-self-model",
    label: "GET /cognition/self-model",
    intervalMs: POLL_MS,
  });
}

export function usePrecedents(): Resource<Precedents> {
  return useResource<Precedents>(cognition.precedents, {
    key: "cognition-precedents",
    label: "GET /cognition/precedents",
    intervalMs: POLL_MS,
  });
}

// --- helpers the pages share ---------------------------------------------------------

/** The fields a precedent is read by first, when the memory answered them. */
export const PRECEDENT_LEADING_FIELDS = ["similarity", "outcome", "age"] as const;

/**
 * The keys of a record in display order: the leading three where present,
 * then every other key as it came. Nothing is dropped and nothing is renamed,
 * so a field this console did not anticipate is still on the screen. Applied
 * at every level — a precedent's own keys, the keys of its `digest`, the
 * columns of its `nearest[]` — so `similarity` leads wherever it appears.
 */
export function precedentFields(record: Readonly<Record<string, unknown>>): readonly string[] {
  const keys = Object.keys(record);
  const leading = PRECEDENT_LEADING_FIELDS.filter((field) => keys.includes(field));
  const rest = keys.filter((key) => !(PRECEDENT_LEADING_FIELDS as readonly string[]).includes(key));
  return [...leading, ...rest];
}

/** A plain object — not `null`, not a list. */
export function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * A non-empty list every element of which is a plain object, so it can be
 * drawn as a table. `nearest[]` is one; an empty list is not, because a table
 * with no rows and no columns says nothing, and an empty list is said in
 * words instead.
 */
export function isRecordList(value: unknown): value is readonly Readonly<Record<string, unknown>>[] {
  return Array.isArray(value) && value.length > 0 && value.every((element) => isRecord(element));
}

/**
 * The columns of a list of records: every key any row carries, in
 * first-seen order, the leading fields promoted. `nearest[]` omits
 * `realised_move_bps` and `agreed` on an unresolved episode, so a table that
 * took its columns from the first row alone would drop them for every row.
 */
export function recordListColumns(rows: readonly Readonly<Record<string, unknown>>[]): readonly string[] {
  const union: Record<string, unknown> = {};
  for (const row of rows) {
    for (const key of Object.keys(row)) {
      if (!(key in union)) union[key] = true;
    }
  }
  return precedentFields(union);
}

/** A scalar value as text, as it came; a structure as JSON, for the arm nothing else draws. */
export function precedentValue(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") return String(value);
  return JSON.stringify(value);
}
