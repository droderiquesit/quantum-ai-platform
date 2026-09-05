"use client";

import type { ReactNode } from "react";
import { StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { platform } from "@/lib/api/client";
import type { SystemStatus } from "@/lib/api/types";
import { useResource } from "@/lib/hooks/useResource";

/**
 * What every cognition page carries, and why it is one component.
 *
 * The pages under `/cognition` render the platform's account of itself — the
 * accuracy it has measured for each of its origins, the precedents its memory
 * last recalled — and can change none of it: no control here grades an
 * outcome, re-weights an origin, stores or forgets an episode, and
 * `client.ts` declares no write they could call. That is a fact about the
 * console, so it is stated unconditionally, in the page and not only in the
 * chrome, on every one of them.
 *
 * Beside that statement sits the platform's live capability from
 * `GET /system/status`, read exactly as the treasury pages read it: `PAPER
 * TRADING` when the platform reports it is not live-capable, a red
 * `LIVE-CAPABLE` alarm if it ever reports otherwise. The cognition bodies
 * carry no `posture` literal of their own — the contract has none — so
 * nothing is invented in its place; the chip is the platform's answer and the
 * label is this console's declaration.
 *
 * One component rather than two copies so that a page cannot be added to
 * this section without the declaration: the specs assert it on each page, and
 * a page that dropped this header would fail all of them.
 */
export function CognitionHeader({
  title,
  reads,
  meta,
}: {
  title: string;
  /** The route this page reads, named so an operator knows where a figure came from. */
  reads: string;
  meta?: ReactNode;
}) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: `cognition-status-${title.toLowerCase().replace(/\s+/g, "-")}`,
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="cognition-header">
      <PanelHead
        title={title}
        meta={meta}
        actions={
          status.data === null ? null : (
            <StatusChip
              tone={status.data.live_capable ? "bad" : "ok"}
              label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
              title="GET /system/status: live_capable"
            />
          )
        }
      />
      <PanelBody>
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]" data-testid="cognition-declaration">
          <span className="chip mr-2" data-tone="ok" data-testid="cognition-paper-label">
            PAPER TRADING
          </span>
          Nothing on this page can act. It reads {reads} and renders what the platform measured about
          itself; there is no control here that grades an outcome, re-weights an origin, stores or
          recalls an episode, or composes an order, and the gateway declares no write this page could
          call. The platform is paper-only and this page cannot change that.
        </p>
      </PanelBody>
    </Panel>
  );
}

export function Muted({ children }: { children: ReactNode }) {
  return <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{children}</span>;
}
