"use client";

import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus, SystemView } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The audit trail's health: how much has been logged, whether the chain still
 * verifies, and whether any of it would survive the process.
 *
 * Two reads, because they answer different questions. GET /system reports what
 * the log claims about itself — length, chain integrity, where a break sits if
 * one does. GET /system/status reports what has left the process: `archived`
 * is null when this deployment archives nothing, which is a different fact
 * from zero and is rendered as a warning rather than a number.
 */
export default function AuditTrailPage() {
  const system = useResource<SystemView>(platform.system, {
    key: "audit-system",
    label: "GET /system",
    intervalMs: 10_000,
  });
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "audit-system-status",
    label: "GET /system/status",
    intervalMs: 10_000,
  });

  const archived = status.data?.archived;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Audit trail"
          meta={<Freshness resource={system} name="audit trail" />}
          actions={
            system.data ? (
              // Posture is on this panel, so the paper label is too: the rule
              // is that autonomy is never shown without the trading posture
              // beside it, and live-capable would be an alarm, not a mode.
              <>
                <Chip title={`autonomy ${system.data.autonomy} · ceiling ${system.data.ceiling}`}>
                  autonomy {system.data.autonomy} / {system.data.ceiling}
                </Chip>
                <StatusChip
                  tone={system.data.live ? "bad" : "ok"}
                  label={system.data.live ? "LIVE-CAPABLE" : "PAPER TRADING"}
                />
                {system.data.halted ? (
                  <StatusChip
                    tone="bad"
                    label="halted"
                    title={system.data.halted_scopes.join(", ") || "no scope named"}
                  />
                ) : null}
              </>
            ) : null
          }
        />
        <PanelBody>
          <ResourceView resource={system} loadingRows={3}>
            {(data) => (
              <div className="flex flex-col gap-3">
                <KpiRow>
                  <Kpi
                    label="Events logged"
                    value={formatCount(data.events_logged)}
                    note="records in the hash chain"
                  />
                  <Kpi
                    label="Archived"
                    value={
                      status.data === null
                        ? "—"
                        : archived === null
                          ? "in-memory only"
                          : formatCount(archived)
                    }
                    tone={status.data === null ? "neutral" : archived === null ? "warn" : "ok"}
                    note={
                      status.data === null
                        ? "reading GET /system/status"
                        : archived === null
                          ? "nothing has left the process"
                          : "records written past the process boundary"
                    }
                  />
                  <Kpi
                    label="Chain intact"
                    value={data.chain_intact ? "yes" : "NO"}
                    tone={data.chain_intact ? "ok" : "bad"}
                    note={
                      data.chain_intact
                        ? "every link re-verified on this read"
                        : "verification failed — see below"
                    }
                  />
                  <Kpi label="Cycles" value={formatCount(data.cycles)} note="loop iterations logged" />
                </KpiRow>

                {!data.chain_intact ? (
                  <StateBlock
                    tone="bad"
                    label="chain broken"
                    headline={
                      data.chain_broken_at === null
                        ? "The event log's hash chain failed verification."
                        : `The event log's hash chain breaks at record ${formatCount(data.chain_broken_at)}.`
                    }
                  >
                    <p>
                      Every record after the break is unverifiable: its links descend from a hash
                      that does not match what was sealed. Nothing decided since that record can be
                      attributed from the log alone. This is the condition the chain exists to make
                      loud — treat the log as compromised until the break is explained.
                    </p>
                  </StateBlock>
                ) : null}
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Why this page watches two numbers" />
        <PanelBody>
          <div className="max-w-[80ch] flex flex-col gap-2 text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
            <p>
              The event log is hash-chained: each record seals the hash of the one before it, so a
              record cannot be edited after sealing without breaking every subsequent link. That is
              what <span className="num">chain_intact</span> verifies live on every read of this
              page — not a stored attestation, but the chain walked again.
            </p>
            <p>
              Integrity is only half of survivability, which is why{" "}
              <span className="num">archived</span> sits beside it. A chain can verify perfectly and
              still exist only in this process&rsquo;s memory; &ldquo;archived: null&rdquo; means
              exactly that, and an unarchived log dies with the process. A crash at the wrong moment
              would leave every decision since start-up unattributable, however intact the chain was
              the instant before.
            </p>
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}
