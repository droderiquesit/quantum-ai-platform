"use client";

import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus, Risk, Autonomy, Governance } from "@/lib/api/types";
import { formatCount, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * M6 Policy Status: governance, autonomy, and control posture.
 *
 * This page displays the platform's current policy state: autonomy level,
 * kill switch status, risk limit utilisation, and governance findings.
 * All data comes directly from the platform's REST surface.
 *
 * **No control on this page.** The kill switch can be tripped from the risk
 * page; everything else is read-only. The PAPER TRADING label is always
 * visible to confirm the autonomy ceiling.
 */

function autonomyTone(level: string): "ok" | "warn" | "bad" | "neutral" {
  if (level.includes("live")) return "bad";
  if (level.includes("supervised")) return "warn";
  return "ok";
}

function autonomyDetails(level: string): string {
  if (level === "paper_trading") return "Paper trading only. No live orders are possible.";
  if (level.includes("supervised_live")) return "Supervised live trading. Orders require manual approval.";
  if (level.includes("autonomous_live")) return "Autonomous live trading. Orders execute automatically.";
  return "Unknown autonomy level. Check configuration.";
}

export default function PolicyStatusPage() {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "policy-status",
    label: "GET /system/status",
    intervalMs: 10_000,
  });

  const autonomy = useResource<Autonomy>(platform.autonomy, {
    key: "policy-autonomy",
    label: "GET /autonomy",
    intervalMs: 15_000,
  });

  const risk = useResource<Risk>(platform.risk, {
    key: "policy-risk",
    label: "GET /risk",
    intervalMs: 20_000,
  });

  const governance = useResource<Governance>(platform.governance, {
    key: "policy-governance",
    label: "GET /governance",
    intervalMs: 30_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="policy-page">
      {/* Header */}
      <Panel>
        <PanelHead
          title="Policy status"
          meta={<Freshness resource={status} name="system" />}
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
          <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
            <span className="chip mr-2" data-tone="ok" data-testid="policy-paper-label">
              PAPER TRADING
            </span>
            The platform's governance, autonomy, and risk control posture. Every figure comes from
            the platform's own state; none are inferred or computed here. No control on this page
            modifies policy — configuration changes require operator credentials and come through
            dedicated endpoints.
          </p>
        </PanelBody>
      </Panel>

      {/* Autonomy and platform state */}
      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Autonomy level"
            meta={<Freshness resource={autonomy} name="autonomy" />}
            actions={<Chip>GET /api/v1/autonomy</Chip>}
          />
          <PanelBody>
            <ResourceView resource={autonomy} loadingRows={4}>
              {(data) => (
                <div className="flex flex-col gap-3">
                  <div className="flex items-center gap-2">
                    <Chip tone={autonomyTone(data.level)}>{data.level}</Chip>
                    <span className="text-[11px] text-[color:var(--color-ink-faint)]">current level</span>
                  </div>
                  <div>
                    <span className="eyebrow text-[11px]">ceiling configured</span>
                    <p className="mt-1 num text-[14px] font-medium text-[color:var(--color-ink)]">
                      {data.ceiling}
                    </p>
                  </div>
                  <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                    {autonomyDetails(data.level)}
                  </p>
                  {data.history && data.history.length > 0 ? (
                    <div className="pt-2 border-t border-border">
                      <span className="eyebrow text-[11px]">latest change</span>
                      <div className="mt-2 flex flex-col gap-1">
                        <p className="text-[11px] text-[color:var(--color-ink-dim)]">
                          {data.history[0].from} → {data.history[0].to}
                        </p>
                        <p className="text-[10.5px] text-[color:var(--color-ink-faint)]">
                          {formatTimestamp(new Date(data.history[0].at * 1000).toISOString())}
                        </p>
                        {data.history[0].reason && (
                          <p className="text-[10.5px] italic text-[color:var(--color-ink-dim)]">
                            "{data.history[0].reason}"
                          </p>
                        )}
                      </div>
                    </div>
                  ) : null}
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="System posture"
            meta={<Freshness resource={status} name="system" />}
            actions={<Chip>GET /api/v1/system/status</Chip>}
          />
          <PanelBody>
            <ResourceView resource={status} loadingRows={4}>
              {(data) => (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi
                      label="Halted"
                      value={data.halted ? "yes" : "no"}
                      tone={data.halted ? "bad" : "ok"}
                      note={data.halted ? "kill switch is tripped" : "platform is running"}
                    />
                    <Kpi
                      label="Cycles"
                      value={formatCount(data.cycles)}
                      note="completed loop iterations"
                    />
                    <Kpi
                      label="Events logged"
                      value={formatCount(data.events)}
                      note="total event record length"
                    />
                  </KpiRow>

                  {data.halted && data.halted_scopes.length > 0 ? (
                    <div className="pt-2 border-t border-border">
                      <span className="eyebrow text-[11px]">halted scopes</span>
                      <div className="mt-1.5 flex flex-wrap gap-1">
                        {data.halted_scopes.map((scope) => (
                          <Chip key={scope} tone="bad">
                            {scope}
                          </Chip>
                        ))}
                      </div>
                    </div>
                  ) : null}
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      {/* Risk controls */}
      <Panel>
        <PanelHead
          title="Risk control status"
          meta={<Freshness resource={risk} name="risk" />}
          actions={<Chip>GET /api/v1/risk</Chip>}
        />
        <PanelBody>
          <ResourceView resource={risk} loadingRows={5}>
            {(data) => (
              <div className="flex flex-col gap-3">
                {/* Exposure */}
                <div className="border-b border-border pb-3">
                  <span className="eyebrow">exposure limits</span>
                  {typeof data.exposure === "object" && data.exposure !== null && "available" in data.exposure && data.exposure.available === true ? (
                    <div className="mt-2 flex flex-col gap-2">
                      <p className="text-[11.5px] text-[color:var(--color-ink-dim)]">
                        {(data.exposure as any).buckets?.length ?? 0} exposure axes monitored
                      </p>
                      <div className="flex flex-wrap gap-2">
                        <Chip tone="ok">monitored</Chip>
                      </div>
                    </div>
                  ) : (
                    <div className="mt-2">
                      <StateBlock
                        tone="neutral"
                        label="not available"
                        headline="Exposure data not available."
                        compact
                      >
                        <p className="text-[11px]">The platform holds no exposure breakdown.</p>
                      </StateBlock>
                    </div>
                  )}
                </div>

                {/* Concentration */}
                <div className="border-b border-border pb-3">
                  <span className="eyebrow">concentration limits</span>
                  {typeof data.concentrations === "object" && data.concentrations !== null && "available" in data.concentrations && data.concentrations.available === true ? (
                    <div className="mt-2 flex flex-col gap-2">
                      <p className="text-[11.5px] text-[color:var(--color-ink-dim)]">
                        {(data.concentrations as any).findings?.length ?? 0} findings
                      </p>
                    </div>
                  ) : (
                    <div className="mt-2">
                      <StateBlock
                        tone="neutral"
                        label="not available"
                        headline="Concentration data not available."
                        compact
                      >
                        <p className="text-[11px]">The platform holds no concentration findings.</p>
                      </StateBlock>
                    </div>
                  )}
                </div>

                {/* Kill switch */}
                <div>
                  <span className="eyebrow">kill switch</span>
                  <div className="mt-2 flex flex-col gap-2">
                    <Chip tone={data.kill_switch.halted ? "bad" : "ok"}>
                      {data.kill_switch.halted ? "TRIPPED" : "armed"}
                    </Chip>
                    {data.kill_switch.halted && (
                      <div className="flex flex-col gap-1.5 border-l-2 border-red-500/50 pl-3">
                        <p className="text-[11px] text-[color:var(--color-ink-dim)]">
                          Tripped by: {data.kill_switch.tripped_by}
                        </p>
                        <p className="text-[11px] text-[color:var(--color-ink-dim)]">
                          Reason: {data.kill_switch.reason}
                        </p>
                        <p className="text-[10.5px] text-[color:var(--color-ink-faint)]">
                          Clearances on record: {formatCount(data.kill_switch.clearances)}
                        </p>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      {/* Governance findings */}
      <Panel>
        <PanelHead
          title="Governance findings"
          meta={<Freshness resource={governance} name="governance" />}
          actions={<Chip>GET /api/v1/governance</Chip>}
        />
        <PanelBody>
          <ResourceView resource={governance} loadingRows={3}>
            {(data) => {
              const errors = data.findings.filter((f) => f.severity === "error");
              const warnings = data.findings.filter((f) => f.severity === "warning");

              return (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi
                      label="Agents"
                      value={formatCount(data.agents)}
                      note="registered and monitored"
                    />
                    <Kpi
                      label="Errors"
                      value={formatCount(errors.length)}
                      tone={errors.length > 0 ? "bad" : "ok"}
                      note="policy violations"
                    />
                    <Kpi
                      label="Warnings"
                      value={formatCount(warnings.length)}
                      tone={warnings.length > 0 ? "warn" : "ok"}
                      note="anomalous findings"
                    />
                  </KpiRow>

                  {data.findings.length === 0 ? (
                    <StateBlock
                      tone="ok"
                      label="clean"
                      headline="No governance findings."
                      compact
                    >
                      <p>All agents are operating within policy.</p>
                    </StateBlock>
                  ) : (
                    <div className="mt-2 space-y-2 border-t border-border pt-3">
                      {data.findings.map((finding, idx) => (
                        <div
                          key={idx}
                          className={`p-2 rounded border-l-2 ${
                            finding.severity === "error"
                              ? "border-red-500/50 bg-red-500/5"
                              : "border-amber-500/50 bg-amber-500/5"
                          }`}
                        >
                          <p className="text-[11px] font-medium text-[color:var(--color-ink)]">
                            {finding.rule}
                          </p>
                          <p className="text-[10.5px] text-[color:var(--color-ink-dim)] mt-1">
                            {finding.detail}
                          </p>
                          {finding.agents.length > 0 && (
                            <div className="mt-1.5 flex flex-wrap gap-1">
                              {finding.agents.map((agent) => (
                                <Chip key={agent} tone="neutral">
                                  {agent}
                                </Chip>
                              ))}
                            </div>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              );
            }}
          </ResourceView>
        </PanelBody>
      </Panel>

      {/* Policy documentation */}
      <Panel>
        <PanelHead title="About policy status" />
        <PanelBody>
          <div className="space-y-3 max-w-[86ch]">
            <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
              <span className="font-semibold">Autonomy level:</span> The degree to which the platform can act
              without human intervention. Paper trading is the only option until a human operator
              changes the configuration. No code in this platform can escalate autonomy on its own.
            </p>
            <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
              <span className="font-semibold">Risk controls:</span> Pre-trade checks that run before an order
              exists. These include exposure limits, concentration limits, and feasibility gates. A
              control that cannot fire is a defect, not a spare part.
            </p>
            <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
              <span className="font-semibold">Kill switch:</span> The emergency halt mechanism. Tripping it
              stops all trading immediately. Clearing it requires operator credentials verified
              within the last fifteen minutes and cannot be done from this page.
            </p>
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}
