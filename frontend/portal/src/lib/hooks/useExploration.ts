"use client";

import { request } from "@/lib/api/client";
import type { Section } from "@/lib/api/types";
import { useResource, type Resource } from "./useResource";

/**
 * §40.1's exploration surface: what the platform is spending to learn, read
 * from `GET /exploration` and rendered as answered.
 *
 * The shape is transcribed field for field from the `exploration` handler in
 * `backend/crates/apps/qip-api/src/routes.rs` — not from the kernel types it
 * reads, because the handler deliberately serialises less than they hold: a
 * `Probe` carries the `score` that selected it, and the route does not send
 * it, so nothing here types it. A field this file names that the wire does not
 * carry would render as a blank the reader takes for a measured nothing.
 *
 * **There is no control here, and there is no fetcher that could make one.**
 * The "Acts on" cell of §40.1's Exploration row is "adjust the exploration
 * share"; the share is a term of a `Mandate`, and a mandate changes through
 * the capital path under an authenticated operator, never through a viewer's
 * GET. This module declares one read and the gateway refuses any write
 * `@/lib/api/endpoints` does not name.
 *
 * Nothing in this file adds, divides or defaults. Held, committed and spent
 * arrive as three decimal strings and are rendered as three; summing any two
 * double-counts an open probe, which is the handler's own reason for keeping
 * them apart. The share is rendered where the platform declared one and its
 * absence is rendered where it did not, because a ledger with no desk mandate
 * has no exploration ceiling and a default shown here would be a number the
 * capital path would refuse.
 */

const POLL_MS = 30_000;

/** A `qip_core::Decimal`, serialised as its exact decimal string. Never parsed here. */
export type DecimalString = string;

/** A `qip_core::Timestamp`, serialised as RFC 3339. Rendered, never recomputed. */
export type Rfc3339 = string;

/**
 * The desk mandate's declared share, when the ledger holds a desk mandate.
 * Otherwise the route answers {@link Unavailable} under the subject
 * `exploration_share`, with the reason, and never a default.
 */
export interface DeclaredShare {
  readonly declared: true;
  /** `Mandate::exploration_share`, a fraction of the desk's capital, as the mandate states it. */
  readonly share: DecimalString;
}

/**
 * One open probe: the question the platform bought, at what bound, against
 * what uncertainty, and until when.
 *
 * `kind` is the `ProbeKind` token and `learns` is `ProbeKind::learns` — the
 * question in words — sent side by side so a reader sees the question rather
 * than an enum they would have to look up.
 */
export interface OpenProbe {
  readonly id: string;
  readonly kind: string;
  readonly learns: string;
  readonly subject: string;
  /** The most the probe may cost. Money, so a decimal string. */
  readonly maximum_loss: DecimalString;
  /**
   * The uncertainty the subject carried when the probe opened — the baseline
   * its information gain is measured against. A statistic, not money, and the
   * one field on a probe the route sends as a JSON number.
   */
  readonly uncertainty_at_open: number;
  readonly opened_at: Rfc3339;
  readonly expires_at: Rfc3339;
}

export interface Exploration {
  readonly share: Section<DeclaredShare>;
  /** What the reservation ledger is withholding from return-seeking capital. */
  readonly held: DecimalString;
  /** Capital a probe still has open against it. */
  readonly committed: DecimalString;
  /** What exploration has actually cost. */
  readonly spend: DecimalString;
  readonly open_count: number;
  readonly opened_total: number;
  readonly settled_total: number;
  readonly abandoned_total: number;
  /** Subjects the book has forgotten under its bounded retention. */
  readonly subjects_forgotten: number;
  readonly open: readonly OpenProbe[];
  /**
   * The per-kind "what it learned" table, or the route's stated absence.
   *
   * The served arm is transcribed from the `exploration` handler's
   * `learned` object, which walks `ProbeKind::ALL` and emits one row per
   * kind whether or not the book holds a record for it. The route's own test
   * (`the_exploration_surface_keeps_what_is_held_committed_and_spent_as_three_numbers`)
   * pins the row count to the enum's, so a kind missing from this table is a
   * wire the route did not send, not a kind with nothing to report. The
   * absence arm is kept because `Section` is the convention every surface
   * answers under, and a body this console does not recognise is still
   * rendered verbatim rather than through guessed columns.
   */
  readonly learned: Section<LearnedTable>;
}

/**
 * The served table: every `ProbeKind`, in the enum's declaration order.
 *
 * `available` is stated as `true` rather than implied by the presence of
 * `kinds`, mirroring how the route states `false` on an absence: a client
 * that forgot to check would otherwise read a missing key as an empty table.
 */
export interface LearnedTable {
  readonly available: true;
  readonly kinds: readonly LearnedKind[];
}

/**
 * One row of the table: `KindRecord`'s fields, with the kind's token and the
 * question it buys beside them. Field names are the handler's, transcribed.
 *
 * Two fields are statistics and arrive as JSON numbers; one is money and
 * arrives as a decimal string; and one may be `null`. That `null` is the
 * point of the row: `measured_gain` is the mean information gain per probed
 * settlement, and where nothing has been probed there is nothing to average.
 * The route sends `null` there, never `0`, because "not measured" and
 * "measured as nothing" are different statements — and this console renders
 * the `null` as the words, never as a figure.
 */
export interface LearnedKind {
  readonly kind: string;
  readonly learns: string;
  readonly opened: number;
  /** Settlements on probed evidence — the only ones that feed the reward. */
  readonly probed: number;
  /** Settlements on observed evidence, counted and reported, never scored. */
  readonly observed: number;
  /** The sum of information gains on probed evidence. A statistic. */
  readonly probed_gain: number;
  /** Everything this kind has actually spent. Money, so a decimal string. */
  readonly realised_cost: DecimalString;
  /** Settlements whose cost exceeded the probe's own bound. */
  readonly bound_breaches: number;
  /** The mean gain per probed settlement, or `null` where nothing has been probed. */
  readonly measured_gain: number | null;
}

/** The one read, and the only one. See the module note. */
export const exploration = {
  read: (signal?: AbortSignal) => request<Exploration>("/exploration", signal ? { signal } : {}),
} as const;

export function useExploration(): Resource<Exploration> {
  return useResource<Exploration>(exploration.read, {
    key: "exploration",
    label: "GET /exploration",
    intervalMs: POLL_MS,
  });
}

/**
 * A statistic as the platform sent it.
 *
 * Deliberately not rounded: `uncertainty_at_open` is an `f64` on the wire,
 * and `String(value)` is the shortest text that reads back as the same
 * double. An uncertainty shown to two places would tell a reader the probe
 * was opened against a baseline it was not.
 */
export function formatStatistic(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return String(value);
}
