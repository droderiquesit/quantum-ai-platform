"use client";

import { useSyncExternalStore } from "react";
import type { CycleReport } from "@/lib/api/types";

/**
 * The cycle reports this console has been handed, in this tab.
 *
 * The platform serves a cycle's eight stage outcomes — what each produced and
 * the sentence it wrote about it — in exactly one place: the response to the
 * `POST /cycle` that ran it. No route serves a past cycle's stages, and the
 * operator interface's own `CycleOverview` is read into HTML, not JSON. So a
 * page that wants to show the stages of the latest cycle without being the
 * page that runs cycles has one honest source: the report the run control
 * received, kept by this console and labelled as such.
 *
 * Kept in `sessionStorage` rather than component state so the report survives
 * a navigation from the loop page to the dataflow page, and *only* in
 * `sessionStorage` so it does not survive the tab: a report shown in a fresh
 * tab a week later would describe a process that has restarted twice since.
 * The reader compares the report's cycle number against the platform's own
 * count and says when the two disagree.
 *
 * Bounded. A console left running all day must not grow a record per cycle.
 */

export interface CycleRun {
  readonly report: CycleReport;
  /** Wall-clock time the response landed. */
  readonly at: number;
}

const STORAGE_KEY = "algorik.cycle-reports";
const BOUND = 24;

const EMPTY: readonly CycleRun[] = [];
let runs: readonly CycleRun[] = EMPTY;
let hydrated = false;
const listeners = new Set<() => void>();

function isCycleRun(value: unknown): value is CycleRun {
  if (typeof value !== "object" || value === null) return false;
  const record = value as { report?: unknown; at?: unknown };
  if (typeof record.at !== "number") return false;
  if (typeof record.report !== "object" || record.report === null) return false;
  const report = record.report as { cycle?: unknown; stages?: unknown };
  return typeof report.cycle === "number" && Array.isArray(report.stages);
}

/** Read what an earlier page in this tab stored, once, on the client. */
function hydrate(): void {
  if (hydrated) return;
  hydrated = true;
  if (typeof window === "undefined") return;
  try {
    const raw = window.sessionStorage.getItem(STORAGE_KEY);
    if (raw === null) return;
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return;
    // Anything that is not a report is dropped rather than rendered: a stale
    // shape from an older build must not reach the page as a cycle.
    runs = parsed.filter(isCycleRun).slice(-BOUND);
  } catch {
    runs = EMPTY;
  }
}

function persist(): void {
  if (typeof window === "undefined") return;
  try {
    window.sessionStorage.setItem(STORAGE_KEY, JSON.stringify(runs));
  } catch {
    // Storage full or disabled: the in-memory copy still serves this page
    // load, and the next page load says honestly that it holds no report.
  }
}

/** Record a report the platform returned to this console. Newest last. */
export function recordCycleReport(report: CycleReport, at: number): void {
  hydrate();
  runs = [...runs, { report, at }].slice(-BOUND);
  persist();
  for (const notify of listeners) notify();
}

function subscribe(listener: () => void): () => void {
  hydrate();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): readonly CycleRun[] {
  hydrate();
  return runs;
}

function getServerSnapshot(): readonly CycleRun[] {
  return EMPTY;
}

/** Every report this tab holds, oldest first. Empty on the server. */
export function useCycleReports(): readonly CycleRun[] {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
