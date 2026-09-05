"use client";

import type { ReactNode } from "react";
import { Chip, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus } from "@/lib/api/types";
import { useResource } from "@/lib/hooks/useResource";
import type { Capability } from "@/lib/hooks/useTreasury";

/**
 * What every treasury page carries, and why it is one component.
 *
 * The four pages under `/treasury` render the ledger plane and can move
 * nothing: no proposal, approval, signature or transfer control exists on any
 * of them, and `client.ts` declares no write they could call. That is a fact
 * about the console, so it is stated unconditionally, in the page and not
 * only in the chrome, on every one of them.
 *
 * Two postures sit beside that statement, both reports and neither an
 * assumption. The body's own `posture` literal — every treasury route answers
 * `"PAPER TRADING"` as its first key, and the contract says render it — is
 * shown as it came, so a body that ever said something else would be shown
 * saying it. And the platform's live capability from `GET /system/status`,
 * read exactly as the dataflow page reads it: `PAPER TRADING` when the
 * platform reports it is not live-capable, a red `LIVE-CAPABLE` alarm if it
 * ever reports otherwise.
 *
 * One component rather than four copies so that a page cannot be added to
 * this section without the declaration: the specs assert it on each page, and
 * a page that dropped this header would fail all of them.
 */
export function TreasuryHeader({
  title,
  reads,
  posture,
  meta,
}: {
  title: string;
  /** The routes this page reads, named so an operator knows where a figure came from. */
  reads: string;
  /** The body's own `posture` literal, once it has landed. */
  posture: string | null;
  meta?: ReactNode;
}) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: `treasury-status-${title.toLowerCase().replace(/\s+/g, "-")}`,
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="treasury-header">
      <PanelHead
        title={title}
        meta={meta}
        actions={
          <>
            {posture === null ? null : (
              <Chip tone={posture === "PAPER TRADING" ? "ok" : "bad"} title={`${reads}: posture`}>
                <span data-testid="treasury-body-posture">{posture}</span>
              </Chip>
            )}
            {status.data === null ? null : (
              <StatusChip
                tone={status.data.live_capable ? "bad" : "ok"}
                label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
                title="GET /system/status: live_capable"
              />
            )}
          </>
        }
      />
      <PanelBody>
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]" data-testid="treasury-declaration">
          <span className="chip mr-2" data-tone="ok" data-testid="treasury-paper-label">
            PAPER TRADING
          </span>
          Nothing on this page can move capital. It reads {reads} and renders what the platform
          answered; there is no control here that proposes, approves, signs or transfers, and the
          gateway declares no write this page could call. ADR 0021 refuses the half of the treasury
          by which capital leaves the platform and ADR 0023 keeps that in force.
        </p>
      </PanelBody>
    </Panel>
  );
}

/** A `Capability` as the platform decided it, with its basis or reason. */
export function CapabilityChip({ label, capability }: { label: string; capability: Capability }) {
  return (
    <div className="flex flex-col gap-0.5">
      <div className="flex items-center gap-1.5">
        <span className="eyebrow">{label}</span>
        <Chip tone={capability.granted ? "ok" : "warn"}>{capability.granted ? "granted" : "refused"}</Chip>
      </div>
      <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{capability.reason}</span>
    </div>
  );
}

/**
 * The withdrawal arm, rendered as refused with the platform's reason.
 *
 * The platform's `WithdrawalEntitlement` has one variant and the route says
 * `granted` is always `false`. This is still a report, not an assumption: if
 * a body ever carried `granted: true` here the page would not quietly show it
 * as refused — it would raise the contradiction as an alarm, because a
 * console that rendered the safe answer over the platform's actual answer
 * would be the console that hid the day the boundary moved.
 */
export function WithdrawalChip({ entitlement }: { entitlement: Capability }) {
  if (entitlement.granted) {
    return (
      <div className="flex flex-col gap-0.5" data-testid="withdrawal-entitlement" data-alert="true" role="alert">
        <div className="flex items-center gap-1.5">
          <span className="eyebrow">can withdraw</span>
          <Chip tone="bad">GRANTED — CONTRADICTS ADR 0021</Chip>
        </div>
        <span className="text-[11px] leading-snug text-[color:var(--color-down)]">
          The platform answered a granted withdrawal, which its own type cannot hold. Stop and
          investigate the process serving this route. Basis given: {entitlement.reason}
        </span>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-0.5" data-testid="withdrawal-entitlement">
      <div className="flex items-center gap-1.5">
        <span className="eyebrow">can withdraw</span>
        <Chip tone="bad">refused</Chip>
      </div>
      <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{entitlement.reason}</span>
    </div>
  );
}

/** A subsystem the platform said it does not hold, in its own words, under the caller's label. */
export function AbsentBlock({
  label,
  headline,
  reason,
  testId,
}: {
  label: string;
  headline: string;
  reason: string | null;
  testId?: string;
}) {
  return (
    <div data-testid={testId}>
      <StateBlock tone="warn" label={label} headline={headline}>
        <p>{reason ?? "The platform gave no reason."}</p>
      </StateBlock>
    </div>
  );
}

export function Muted({ children }: { children: ReactNode }) {
  return <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{children}</span>;
}

/** Text from the platform, rendered as it came. */
export function Quote({ children, tone }: { children: ReactNode; tone?: "warn" | "bad" }) {
  const colour =
    tone === "bad" ? "var(--color-down)" : tone === "warn" ? "var(--color-warn)" : "var(--color-ink-dim)";
  return (
    <p className="text-[11.5px] leading-relaxed" style={{ color: colour }}>
      {children}
    </p>
  );
}
