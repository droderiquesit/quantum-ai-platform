/**
 * Deterministic simulation, for the pages whose subsystem does not exist yet.
 *
 * Some destinations in the console's map have nothing behind them — no
 * endpoint, no declared absence, nothing. The honest options are an empty page
 * or an illustration that says it is one. This module is the second option,
 * under three rules:
 *
 * 1. **Deterministic.** Every value comes from a seeded generator keyed on a
 *    stable string. The same page shows the same figures on every load, on
 *    every machine — a "simulation" that reshuffles per refresh would invite
 *    the reader to see movement where there is none.
 * 2. **Labelled at the point of reading.** Any panel fed from here renders
 *    `SimulatedBanner` (or at minimum `SimChip`) from `Simulated.tsx`. A
 *    simulated figure that could be screenshotted without its label is a
 *    fabrication with extra steps.
 * 3. **Shaped like the contract.** Simulated data conforms to the TypeScript
 *    interface the real endpoint will serve, so the page is a working client
 *    of a platform surface that does not exist yet — swap the adapter, keep
 *    the page.
 *
 * Never used for anything the platform actually serves. A real number and a
 * simulated one must never share a panel without each being marked.
 */

/** FNV-1a, folding a string key to a 32-bit seed. */
function fold(key: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < key.length; index++) {
    hash ^= key.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/** mulberry32 — small, fast, and good enough for illustration, keyed. */
export function seeded(key: string): () => number {
  let state = fold(key);
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let mix = Math.imul(state ^ (state >>> 15), 1 | state);
    mix = (mix + Math.imul(mix ^ (mix >>> 7), 61 | mix)) ^ mix;
    return ((mix ^ (mix >>> 14)) >>> 0) / 4294967296;
  };
}

/** A bounded random walk, oldest first. */
export function simWalk(
  key: string,
  points: number,
  options: { start: number; drift?: number; volatility?: number; floor?: number } ,
): number[] {
  const { start, drift = 0, volatility = 0.02, floor = 0 } = options;
  const next = seeded(key);
  const values: number[] = [];
  let value = start;
  for (let index = 0; index < points; index++) {
    value = Math.max(floor, value * (1 + drift + (next() - 0.5) * 2 * volatility));
    values.push(value);
  }
  return values;
}

/** A stable choice from a list. */
export function simPick<T>(key: string, options: readonly T[]): T {
  const index = Math.floor(seeded(key)() * options.length);
  return options[Math.min(index, options.length - 1)] as T;
}

/** A stable number in [lo, hi). */
export function simBetween(key: string, lo: number, hi: number): number {
  return lo + seeded(key)() * (hi - lo);
}
