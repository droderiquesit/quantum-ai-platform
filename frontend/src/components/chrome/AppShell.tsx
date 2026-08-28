"use client";

import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { Chip } from "@/components/data/Bits";
import { formatClock, formatCount, formatUtcDate } from "@/lib/format";
import { useConnections } from "@/lib/hooks/connections";
import { useNow } from "@/lib/hooks/useNow";
import { navItemFor } from "@/lib/nav";
import { CommandPalette } from "./CommandPalette";
import { ConnectionIndicator } from "./ConnectionIndicator";
import { KillSwitch } from "./KillSwitch";
import { NavBar, NavRail } from "./Nav";
import { PaperTradingBanner } from "./PaperTradingBanner";
import { PlatformProvider, usePlatform } from "./PlatformProvider";

export function AppShell({ children }: { children: ReactNode }) {
  return (
    <PlatformProvider>
      <div className="flex h-dvh flex-col overflow-hidden bg-[color:var(--color-void)]">
        <a
          href="#content"
          className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-[100] focus:border focus:border-[color:var(--color-accent)] focus:bg-[color:var(--color-surface)] focus:px-3 focus:py-1.5 focus:text-[12px]"
        >
          Skip to content
        </a>
        <PaperTradingBanner />
        <Header />
        <NavBar />
        <div className="flex min-h-0 flex-1">
          <NavRail />
          <main id="content" tabIndex={-1} className="min-w-0 flex-1 overflow-auto">
            {children}
          </main>
        </div>
        <StatusBar />
      </div>
    </PlatformProvider>
  );
}

function Header() {
  const pathname = usePathname();
  const item = navItemFor(pathname);

  return (
    <header className="flex h-[44px] shrink-0 items-center gap-3 border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)] px-3">
      <div className="flex items-baseline gap-2">
        <span className="num text-[13px] font-semibold tracking-[0.14em] text-[color:var(--color-ink)]">
          QIP
        </span>
        <span className="num text-[10px] tracking-[0.16em] text-[color:var(--color-ink-faint)]">
          COMMAND CENTRE
        </span>
      </div>
      <span className="h-[18px] w-px bg-[color:var(--color-line-strong)]" aria-hidden="true" />
      <h1 className="truncate text-[12.5px] font-medium text-[color:var(--color-ink)]">
        {item?.label ?? "Not found"}
      </h1>
      <span className="hidden truncate text-[11px] text-[color:var(--color-ink-faint)] xl:inline">
        {item?.description ?? ""}
      </span>
      <div className="ml-auto flex items-center gap-2">
        <UtcClock />
        <ConnectionIndicator />
        <CommandPalette />
        <KillSwitch />
      </div>
    </header>
  );
}

function UtcClock() {
  const now = useNow();
  return (
    <div
      className="hidden flex-col items-end leading-none md:flex"
      title="Wall clock, UTC. Every timestamp on this console is UTC."
    >
      <span className="num text-[12px] text-[color:var(--color-ink)]">
        {now === null ? "--:--:--" : formatClock(now).slice(0, 8)}
      </span>
      <span className="num text-[9px] text-[color:var(--color-ink-faint)]">
        {now === null ? "—" : `${formatUtcDate(now)} UTC`}
      </span>
    </div>
  );
}

function StatusBar() {
  const feeds = useConnections();
  const { status, health } = usePlatform();
  const environment = process.env.NEXT_PUBLIC_QIP_ENVIRONMENT ?? "unset";
  const scopes = status.data?.halted_scopes ?? [];

  return (
    <footer className="flex h-[22px] shrink-0 items-center gap-3 overflow-x-auto border-t border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-3 text-[10px] text-[color:var(--color-ink-faint)]">
      <Chip tone="info">{environment}</Chip>
      <span className="num">gateway /api/gateway → /api/v1</span>
      <span className="num">{feeds.length} feed(s)</span>
      {status.data ? (
        <>
          <span className="num">cycles {formatCount(status.data.cycles)}</span>
          <span className="num">events {formatCount(status.data.events)}</span>
          <span className="num">
            archived {status.data.archived === null ? "not configured" : formatCount(status.data.archived)}
          </span>
          <span className="num">autonomy {status.data.autonomy}</span>
        </>
      ) : (
        <span className="num">platform state unknown</span>
      )}
      {health.data && health.data.reconciliation_breaks > 0 ? (
        <Chip tone="bad">{health.data.reconciliation_breaks} reconciliation break(s)</Chip>
      ) : null}
      {scopes.length > 0 ? <Chip tone="bad">halted: {scopes.join(", ")}</Chip> : null}
      <span className="num ml-auto whitespace-nowrap">paper trading — simulated execution only</span>
    </footer>
  );
}
