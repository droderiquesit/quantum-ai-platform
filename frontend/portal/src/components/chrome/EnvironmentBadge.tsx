"use client";

import { usePlatform } from "./PlatformProvider";

/**
 * Which environment this console is reading, said in colour on every screen.
 *
 * This is a report, not a control. The deployment declares its environment in
 * NEXT_PUBLIC_QIP_ENVIRONMENT and the platform declares its own live
 * capability; the badge renders both and changes neither. There is no control
 * anywhere in this console that could move the platform toward live trading —
 * the platform refuses a live ceiling at start-up, and this surface refuses to
 * imply otherwise.
 *
 * Colour code, fixed: simulation blue, paper purple, staging amber, live red.
 * Red is reachable only by the platform itself reporting live_capable=true,
 * which on this deployment is a defect to be alarmed at, not a mode.
 */
type Mode = "simulation" | "paper" | "staging" | "live-capable";

const COLOUR: Record<Mode, string> = {
  simulation: "var(--color-env-simulation)",
  paper: "var(--color-env-paper)",
  staging: "var(--color-env-staging)",
  "live-capable": "var(--color-env-live)",
};

export function EnvironmentBadge() {
  const { health, status } = usePlatform();
  const liveCapable = health.data?.live_capable ?? status.data?.live_capable ?? null;

  const declared = (process.env.NEXT_PUBLIC_QIP_ENVIRONMENT ?? "").toLowerCase();
  const mode: Mode =
    liveCapable === true
      ? "live-capable"
      : declared === "simulation" || declared === "sim"
        ? "simulation"
        : declared === "staging" || declared === "stage"
          ? "staging"
          : "paper";

  return (
    <span
      className="chip"
      data-testid="environment-badge"
      style={{
        color: COLOUR[mode],
        borderColor: `color-mix(in srgb, ${COLOUR[mode]} 55%, transparent)`,
      }}
      title={
        mode === "live-capable"
          ? "The platform itself reports live_capable = true. Nothing in this console can send a live order, but the process behind it says it could — investigate before trusting anything else on screen."
          : `Environment as declared by this deployment. Read-only: no control here changes it. Posture confirmed by the platform: ${
              liveCapable === false ? "cannot reach a live venue" : "not yet read"
            }.`
      }
    >
      <span className="dot" aria-hidden="true" />
      {mode === "live-capable" ? "LIVE-CAPABLE" : mode.toUpperCase()}
    </span>
  );
}
