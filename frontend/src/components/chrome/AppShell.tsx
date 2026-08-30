"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { AlgorikWordmark } from "@algorik/brand";
import { AccountMenu } from "./AccountMenu";
import { Chip } from "@/components/data/Bits";
import { formatClock, formatCount, formatUtcDate } from "@/lib/format";
import { useConnections } from "@/lib/hooks/connections";
import { useNow } from "@/lib/hooks/useNow";
import { navItemFor } from "@/lib/nav";
import { CommandPalette } from "./CommandPalette";
import { InstallApp } from "./InstallApp";
import { ConnectionIndicator } from "./ConnectionIndicator";
import { EnvironmentBadge } from "./EnvironmentBadge";
import { ThemeToggle } from "./ThemeToggle";
import { KillSwitch } from "./KillSwitch";
import { MobileTabBar } from "./MobileTabBar";
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
        {/* Tablet keeps the scrolling strip; a phone gets thumb-reachable tabs
            at the bottom instead. Both are rendered and one is hidden per
            breakpoint, so a link is always in the accessibility tree. */}
        <NavBar />
        <div className="flex min-h-0 flex-1">
          <NavRail />
          <main id="content" tabIndex={-1} className="min-w-0 flex-1 overflow-auto">
            {children}
          </main>
        </div>
        <StatusBar />
        <MobileTabBar />
        <InstallApp />
      </div>
    </PlatformProvider>
  );
}

function Header() {
  const pathname = usePathname();
  const item = navItemFor(pathname);

  return (
    <header className="flex h-[44px] shrink-0 items-center gap-2 border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)] px-2 sm:gap-3 sm:px-3">
      <div className="flex shrink-0 items-center gap-2">
        {/* The shipped Algorik lockup. On a phone the qualifier is dropped
            before the logo is, because the logo is the thing that tells a
            user which product they are looking at. */}
        <AlgorikWordmark size={17} onDark qualifier="Portal" />
      </div>
      <span
        className="hidden h-[18px] w-px bg-[color:var(--color-line-strong)] sm:block"
        aria-hidden="true"
      />
      <h1 className="min-w-0 truncate text-[12.5px] font-medium text-[color:var(--color-ink)]">
        {item?.label ?? "Not found"}
      </h1>
      <span className="hidden min-w-0 truncate text-[11px] text-[color:var(--color-ink-faint)] xl:inline">
        {item?.description ?? ""}
      </span>
      {/* `shrink-0` on the control cluster is load-bearing: the kill switch is
          the one thing on this console that must be reachable at every width,
          and a flex child without it is the first thing a narrow viewport
          truncates. */}
      <div className="ml-auto flex shrink-0 items-center gap-1.5 sm:gap-2">
        <span className="hidden md:inline-flex">
          <EnvironmentBadge />
        </span>
        {/* Not a badge with a count: a count would need the risk, orders and
            governance reads in the shell on every page. The alerts page owns
            deriving them; the header owns being one tap from it. */}
        <Link
          href="/command/alerts"
          className="btn hidden md:inline-flex"
          data-variant="ghost"
          aria-label="Alerts and incidents"
          title="Alerts and incidents"
        >
          ⚠
        </Link>
        <UtcClock />
        <ConnectionIndicator />
        <AccountMenu />
        <span className="hidden sm:inline-flex">
          <ThemeToggle />
        </span>
        {/* A keyboard-shortcut affordance has no business on a touch phone,
            and at 412px it was the difference between the kill switch on
            screen and off it. */}
        <span className="hidden sm:inline-flex">
          <CommandPalette />
        </span>
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
  const scopes = status.data?.halted_scopes ?? [];

  return (
    <footer className="hidden h-[22px] shrink-0 items-center gap-3 overflow-x-auto border-t border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-3 text-[10px] text-[color:var(--color-ink-faint)] md:flex">
      <EnvironmentBadge />
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
