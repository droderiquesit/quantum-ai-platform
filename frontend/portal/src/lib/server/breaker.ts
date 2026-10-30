/**
 * The gateway's circuit breaker, one per upstream, held in this process.
 *
 * Why it exists (R12-GCP-8119: warm-service clients enforce timeouts *and*
 * breakers). Without one, a platform that has stopped answering costs every
 * console request the full upstream timeout before it learns what the last
 * request already knew. A dashboard re-fetching six panels every few seconds
 * then holds a socket and a timer open per panel per poll against a backend
 * that cannot serve them, and an operator watches spinners for ten seconds
 * where an honest "the gateway has stopped asking" could have been said in
 * one millisecond.
 *
 * Three states, in the usual shape:
 *
 * - **closed** — requests are forwarded; each transport failure (the platform
 *   could not be reached, or did not answer within the timeout) increments a
 *   consecutive-failure count, and any answer at all resets it.
 * - **open** — after `threshold` consecutive failures. Every request is refused
 *   without touching the network until `cooldownMs` has elapsed since the
 *   circuit opened.
 * - **half-open** — once the cool-down has elapsed, exactly **one** request is
 *   admitted as a trial. Every other request arriving while that trial is in
 *   flight is still refused: admitting all of them would be the same stampede
 *   the open state exists to prevent. The trial's outcome closes the circuit
 *   or re-opens it with a fresh cool-down.
 *
 * What counts as a failure is deliberately narrow: only the two outcomes where
 * the platform gave no answer. A 4xx or 5xx the platform *returned* is an
 * answer, passed through untouched by the gateway, and counting it would let a
 * stated absence (`/correlation` answering "not available") trip a breaker in
 * front of every other route.
 *
 * Bounded: keyed by the resolved base URL, which comes from this process's own
 * configuration rather than from a request, so in practice there is one entry.
 * The cap is there anyway so that no future caller that keys on something
 * request-derived can grow the map without limit; the oldest entry is evicted,
 * which at worst forgets a failure count — it never opens a circuit that
 * should be closed.
 *
 * In-process on purpose: each Cloud Run instance learns the platform's state
 * from its own requests. A shared breaker would need a store, and a store is a
 * second thing that can be down.
 */

export interface BreakerSettings {
  /** Consecutive transport failures that open the circuit. */
  readonly threshold: number;
  /** How long an open circuit refuses before admitting one trial. */
  readonly cooldownMs: number;
}

type State =
  | { readonly kind: "closed"; readonly failures: number }
  | { readonly kind: "open"; readonly openedAt: number; readonly failures: number }
  | { readonly kind: "half-open"; readonly failures: number };

/** What `admit` decided. A refusal carries the sentence the gateway returns. */
export type Admission =
  | { readonly admitted: true; readonly trial: boolean }
  | { readonly admitted: false; readonly reason: string; readonly retryAfterMs: number };

/** Most upstreams this process tracks at once. See the module doc. */
export const MAX_TRACKED_UPSTREAMS = 16;

export class CircuitBreakers {
  private readonly states = new Map<string, State>();

  constructor(private readonly now: () => number = Date.now) {}

  /** Whether a request to `key` may be forwarded now. */
  admit(key: string, settings: BreakerSettings): Admission {
    const state = this.states.get(key);
    if (state === undefined || state.kind === "closed") {
      return { admitted: true, trial: false };
    }
    if (state.kind === "half-open") {
      return {
        admitted: false,
        reason:
          `the circuit to the platform is half-open after ${state.failures} consecutive ` +
          "failures, and one trial request is already deciding whether to close it",
        retryAfterMs: settings.cooldownMs,
      };
    }
    const elapsed = this.now() - state.openedAt;
    if (elapsed >= settings.cooldownMs) {
      this.put(key, { kind: "half-open", failures: state.failures });
      return { admitted: true, trial: true };
    }
    return {
      admitted: false,
      reason:
        `the circuit to the platform is open after ${state.failures} consecutive failures; ` +
        `this gateway is not forwarding requests for another ${settings.cooldownMs - elapsed}ms`,
      retryAfterMs: settings.cooldownMs - elapsed,
    };
  }

  /** The platform answered — any status. Closes the circuit. */
  succeeded(key: string): void {
    if (this.states.has(key)) this.put(key, { kind: "closed", failures: 0 });
  }

  /** The platform could not be reached or did not answer in time. */
  failed(key: string, settings: BreakerSettings): void {
    const state = this.states.get(key);
    const failures = (state?.failures ?? 0) + 1;
    // A failed trial re-opens at once, whatever the count: the cool-down
    // elapsed and the platform still did not answer, which is the evidence.
    if (state?.kind === "half-open" || failures >= settings.threshold) {
      this.put(key, { kind: "open", openedAt: this.now(), failures });
    } else {
      this.put(key, { kind: "closed", failures });
    }
  }

  /** The state of one upstream, for tests and diagnostics. */
  stateOf(key: string): State["kind"] {
    return this.states.get(key)?.kind ?? "closed";
  }

  /** How many upstreams are tracked. Never above `MAX_TRACKED_UPSTREAMS`. */
  get size(): number {
    return this.states.size;
  }

  private put(key: string, state: State): void {
    // Delete first so re-insertion moves the key to the newest position, and
    // the eviction below always removes the least recently touched upstream.
    this.states.delete(key);
    this.states.set(key, state);
    while (this.states.size > MAX_TRACKED_UPSTREAMS) {
      const oldest = this.states.keys().next().value;
      if (oldest === undefined) break;
      this.states.delete(oldest);
    }
  }
}

/**
 * The process's breakers. A module-level singleton is what "in-process" means
 * for a route handler: Next.js evaluates this module once per server process.
 */
export const gatewayBreakers = new CircuitBreakers();
