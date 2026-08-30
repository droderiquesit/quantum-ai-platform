"use client";

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { platform, type ApiOutcome } from "@/lib/api/client";
import type { Health, KillSwitchResponse, SystemStatus } from "@/lib/api/types";
import { useResource, type Resource } from "@/lib/hooks/useResource";

/**
 * The platform state the chrome needs on every page.
 *
 * Held once, at the top, for two reasons. Eight panels each polling `/health`
 * would be eight requests a second at the platform's own rate limit for no
 * added information; and more importantly, the halt banner, the connection
 * indicator and the kill switch must agree with each other, which they can only
 * do by reading the same answer.
 */

export interface KillSwitchAction {
  readonly at: number;
  readonly kind: "trip" | "clear";
  readonly outcome: ApiOutcome<KillSwitchResponse>;
}

interface PlatformContextValue {
  readonly health: Resource<Health>;
  readonly status: Resource<SystemStatus>;
  /** True only when the platform said so. Unknown is not false. */
  readonly halted: boolean | null;
  readonly busy: boolean;
  readonly lastAction: KillSwitchAction | null;
  trip(reason: string): Promise<ApiOutcome<KillSwitchResponse>>;
  clear(): Promise<ApiOutcome<KillSwitchResponse>>;
  dismissAction(): void;
}

const PlatformContext = createContext<PlatformContextValue | null>(null);

export function PlatformProvider({ children }: { children: ReactNode }) {
  const health = useResource<Health>(platform.health, {
    key: "health",
    label: "GET /health",
    intervalMs: 5_000,
  });
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "system-status",
    label: "GET /system/status",
    intervalMs: 10_000,
  });

  const [busy, setBusy] = useState(false);
  const [lastAction, setLastAction] = useState<KillSwitchAction | null>(null);

  const refreshHealth = health.refresh;
  const refreshStatus = status.refresh;

  const trip = useCallback(
    async (reason: string) => {
      setBusy(true);
      try {
        const response = await platform.tripKillSwitch(reason);
        setLastAction({ at: Date.now(), kind: "trip", outcome: response.outcome });
        refreshHealth();
        refreshStatus();
        return response.outcome;
      } finally {
        setBusy(false);
      }
    },
    [refreshHealth, refreshStatus],
  );

  const clear = useCallback(async () => {
    setBusy(true);
    try {
      const response = await platform.clearKillSwitch();
      setLastAction({ at: Date.now(), kind: "clear", outcome: response.outcome });
      refreshHealth();
      refreshStatus();
      return response.outcome;
    } finally {
      setBusy(false);
    }
  }, [refreshHealth, refreshStatus]);

  const dismissAction = useCallback(() => setLastAction(null), []);

  const halted = useMemo<boolean | null>(() => {
    if (health.data !== null) return health.data.halted;
    if (status.data !== null) return status.data.halted;
    return null;
  }, [health.data, status.data]);

  const value = useMemo<PlatformContextValue>(
    () => ({ health, status, halted, busy, lastAction, trip, clear, dismissAction }),
    [health, status, halted, busy, lastAction, trip, clear, dismissAction],
  );

  return <PlatformContext.Provider value={value}>{children}</PlatformContext.Provider>;
}

export function usePlatform(): PlatformContextValue {
  const value = useContext(PlatformContext);
  if (value === null) {
    throw new Error("usePlatform must be used inside a PlatformProvider");
  }
  return value;
}
