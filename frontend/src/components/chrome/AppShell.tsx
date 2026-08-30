"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { navItemFor } from "@/lib/nav";
import { AccountMenu } from "./AccountMenu";
import { CommandPalette } from "./CommandPalette";
import { EnvironmentBadge } from "./EnvironmentBadge";
import { Icon } from "./icons";
import { InstallApp } from "./InstallApp";
import { KillSwitch } from "./KillSwitch";
import { Sidebar, useSidebar } from "./Nav";
import { PaperTradingBanner } from "./PaperTradingBanner";
import { PlatformProvider, usePlatform } from "./PlatformProvider";
import { ThemeToggle } from "./ThemeToggle";

/**
 * The portal shell, in the licensed template's structure (ADR 0015): its
 * off-canvas/collapsible sidebar, its glass sticky header, its main-content
 * offset — with the chrome this platform is not allowed to lose rendered
 * inside that structure. The paper-trading banner sits above the header, the
 * environment badge and kill switch live in the header's control cluster, and
 * every testid the behavioural suite relies on is preserved.
 */
export function AppShell({ children }: { children: ReactNode }) {
  return (
    <PlatformProvider>
      <Chrome>{children}</Chrome>
    </PlatformProvider>
  );
}

function Chrome({ children }: { children: ReactNode }) {
  const { collapsed, mobileOpen, setMobileOpen, toggleCollapsed } = useSidebar();

  return (
    <div className="min-h-dvh bg-bg text-text">
      <a
        href="#content"
        className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-[100] focus:rounded-lg focus:border focus:border-accent focus:bg-panel focus:px-3 focus:py-1.5 focus:text-sm"
      >
        Skip to content
      </a>

      <Sidebar collapsed={collapsed} mobileOpen={mobileOpen} onClose={() => setMobileOpen(false)} />

      <main className={`main-content ${collapsed ? "expanded" : ""}`} id="mainContent">
        <PaperTradingBanner />
        <Header
          onMobileMenu={() => setMobileOpen(true)}
          onToggleCollapse={toggleCollapsed}
        />
        <div id="content" tabIndex={-1} className="min-w-0">
          {children}
        </div>
      </main>

      <InstallApp />
    </div>
  );
}

function Header({
  onMobileMenu,
  onToggleCollapse,
}: {
  onMobileMenu: () => void;
  onToggleCollapse: () => void;
}) {
  const pathname = usePathname();
  const item = navItemFor(pathname);
  const { health } = usePlatform();

  const halted = health.data?.halted ?? null;
  const breaks = health.data?.reconciliation_breaks ?? 0;
  const alerting = halted === true || breaks > 0;

  // The template's square control: reused for every header action so the row
  // reads as one system.
  const control =
    "flex size-11 items-center justify-center rounded-xl bg-panel border border-border text-text hover:bg-border/50 transition-colors";

  return (
    <header className="sticky top-0 z-50 glass border-b border-border">
      <div className="flex items-center justify-between px-2 sm:px-6 py-2 sm:py-4 gap-2">
        <div className="flex items-center gap-2 sm:gap-4 min-w-0">
          <button
            type="button"
            className={`lg:hidden ${control}`}
            onClick={onMobileMenu}
            aria-label="Open the navigation"
            data-testid="mobile-menu"
          >
            <Icon name="menu" />
          </button>
          <button
            type="button"
            className={`hidden lg:flex ${control}`}
            onClick={onToggleCollapse}
            aria-label="Collapse or expand the sidebar"
            data-testid="sidebar-collapse"
          >
            <Icon name="panel-left" />
          </button>

          {/* The command palette owns search; this input is its doorway, so
              there are not two competing searches with different answers. */}
          <div className="relative hidden md:block">
            <Icon
              name="search"
              className="w-4 h-4 absolute left-3.5 top-1/2 -translate-y-1/2 text-muted pointer-events-none"
            />
            <input
              type="text"
              readOnly
              placeholder="Search sections, pages… (⌘K)"
              aria-label="Search (opens the command palette)"
              onClick={() =>
                window.dispatchEvent(
                  new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true }),
                )
              }
              className="w-72 xl:w-80 pl-10 pr-4 py-2.5 rounded-xl bg-bg border border-border text-sm text-text placeholder:text-muted cursor-pointer focus:outline-none focus:border-accent"
            />
          </div>

          <h1 className="lg:hidden min-w-0 truncate text-sm font-semibold text-text">
            {item?.label ?? "Algorik"}
          </h1>
        </div>

        <div className="flex items-center gap-2 sm:gap-3 shrink-0">
          {/* The template's LIVE chip, bound to the platform's real halt
              state rather than decoration. */}
          <div
            className={`hidden md:flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs font-bold ${
              halted === null
                ? "bg-border/50 text-muted"
                : halted
                  ? "bg-red-500/10 text-red-500"
                  : "bg-emerald-500/10 text-emerald-500"
            }`}
            title={
              halted === null
                ? "The platform has not answered /health yet."
                : halted
                  ? "The platform is halted."
                  : "The platform is running. Execution is simulated — paper trading."
            }
          >
            <span
              className={`live-dot ${halted ? "!bg-red-500" : halted === null ? "!bg-slate-400" : "!bg-emerald-500"}`}
              aria-hidden="true"
            />
            {halted === null ? "SYNCING" : halted ? "HALTED" : "LIVE · PAPER"}
          </div>

          <span className="hidden md:inline-flex">
            <EnvironmentBadge />
          </span>

          <ThemeToggle />

          <Link
            href="/command/alerts"
            className={`relative ${control}`}
            aria-label="Alerts and incidents"
            title="Alerts and incidents"
          >
            <Icon name="bell" />
            {alerting ? (
              <span
                className="absolute -top-1 -right-1 w-3 h-3 rounded-full bg-red-500 border-2 border-panel"
                aria-hidden="true"
              />
            ) : null}
          </Link>

          <KillSwitch />
          <AccountMenu />
        </div>
      </div>

      {/* Mounted for its ⌘K listener and dialog; its own trigger is hidden —
          the search input above is the visible doorway. */}
      <span className="hidden">
        <CommandPalette />
      </span>
    </header>
  );
}
