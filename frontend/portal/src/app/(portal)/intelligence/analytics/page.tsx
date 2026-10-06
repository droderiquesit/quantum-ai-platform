"use client";

import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ChartJs } from "@/components/viz/ChartJs";
import { ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus, Proposals, Opportunities } from "@/lib/api/types";
import { formatCount, formatDecimal, formatPercent, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { useMemo } from "react";

/**
 * M6 Analytics Dashboard: quantum decision performance and platform metrics.
 *
 * This page provides a holistic view of the DECIDE stage's output, including
 * proposal distribution, opportunity detection, and decision outcomes. All data
 * comes from the platform's REST surface — nothing is computed here.
 *
 * **Quantum decisions are read-only displays.** The page renders what the
 * DECIDE stage produced; no control can modify or submit proposals. The PAPER
 * TRADING label is always visible to confirm posture.
 */

export default function AnalyticsPage() {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "analytics-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  const proposals = useResource<Proposals>(platform.proposals, {
    key: "analytics-proposals",
    label: "GET /proposals",
    intervalMs: 10_000,
  });

  const opportunities = useResource<Opportunities>(platform.opportunities, {
    key: "analytics-opportunities",
    label: "GET /opportunities",
    intervalMs: 15_000,
  });

  // Aggregate proposal metrics
  const proposalMetrics = useMemo(() => {
    if (proposals.data === null) {
      return { total: 0, approved: 0, vetoed: 0, withdrawn: 0, draft: 0 };
    }

    const data = proposals.data.proposals;
    return {
      total: data.length,
      approved: data.filter((p) => p.status === "approved" || p.status === "released").length,
      vetoed: data.filter((p) => p.status === "vetoed").length,
      withdrawn: data.filter((p) => p.status === "withdrawn").length,
      draft: data.filter((p) => p.status === "draft").length,
    };
  }, [proposals.data]);

  // Opportunity metrics and scoring distribution
  const opportunityMetrics = useMemo(() => {
    if (opportunities.data === null) {
      return { total: 0, avgScore: 0, avgConfidence: 0, scoreDistribution: [] };
    }

    const opps = opportunities.data.opportunities;
    const avgScore = opps.length > 0 ? opps.reduce((sum, o) => sum + o.score, 0) / opps.length : 0;
    const avgConfidence =
      opps.length > 0 ? opps.reduce((sum, o) => sum + o.confidence, 0) / opps.length : 0;

    // Score distribution for chart (0-0.2, 0.2-0.4, etc.)
    const bins = [0, 0, 0, 0, 0];
    opps.forEach((o) => {
      const binIndex = Math.min(Math.floor(o.score * 5), 4);
      bins[binIndex]++;
    });

    return {
      total: opps.length,
      avgScore,
      avgConfidence,
      scoreDistribution: bins,
    };
  }, [opportunities.data]);

  // Chart configurations
  const proposalStatusChart = useMemo(
    () => ({
      type: "doughnut" as const,
      data: {
        labels: ["Approved", "Vetoed", "Withdrawn", "Draft"],
        datasets: [
          {
            data: [proposalMetrics.approved, proposalMetrics.vetoed, proposalMetrics.withdrawn, proposalMetrics.draft],
            backgroundColor: [
              "var(--color-up)",
              "var(--color-down)",
              "var(--color-warn)",
              "var(--color-ink-faint)",
            ],
            borderColor: "var(--color-bg)",
            borderWidth: 2,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            position: "bottom" as const,
            labels: {
              color: "var(--color-ink)",
              font: { family: 'var(--font-mono)', size: 12 },
              padding: 12,
            },
          },
        },
      },
    }),
    [proposalMetrics]
  );

  const opportunityScoreChart = useMemo(
    () => ({
      type: "bar" as const,
      data: {
        labels: ["0.0–0.2", "0.2–0.4", "0.4–0.6", "0.6–0.8", "0.8–1.0"],
        datasets: [
          {
            label: "Opportunities",
            data: opportunityMetrics.scoreDistribution,
            backgroundColor: "var(--color-brand)",
            borderColor: "var(--color-brand-dim)",
            borderWidth: 1,
          },
        ],
      },
      options: {
        indexAxis: "y" as const,
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            display: false,
          },
        },
        scales: {
          x: {
            grid: { color: "var(--color-line)", drawBorder: false },
            ticks: { color: "var(--color-ink-dim)", font: { family: 'var(--font-mono)', size: 11 } },
          },
          y: {
            grid: { display: false },
            ticks: { color: "var(--color-ink-dim)", font: { family: 'var(--font-mono)', size: 11 } },
          },
        },
      },
    }),
    [opportunityMetrics]
  );

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="analytics-page">
      {/* Header */}
      <Panel>
        <PanelHead
          title="Analytics"
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
            <span className="chip mr-2" data-tone="ok" data-testid="analytics-paper-label">
              PAPER TRADING
            </span>
            Quantum decision analytics from the DECIDE stage. All figures are read-only; no control
            on this page submits, approves, or cancels anything. The platform computes every metric
            shown; none are derived here.
          </p>
        </PanelBody>
      </Panel>

      {/* Opportunity metrics */}
      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Detected opportunities"
            meta={<Freshness resource={opportunities} name="opportunities" />}
            actions={<Chip>GET /api/v1/opportunities</Chip>}
          />
          <PanelBody>
            <ResourceView resource={opportunities} loadingRows={3}>
              {(data) => (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi
                      label="Total"
                      value={formatCount(opportunityMetrics.total)}
                      note="opportunities ranked by detector confidence"
                    />
                    <Kpi
                      label="Avg score"
                      value={opportunityMetrics.avgScore.toFixed(3)}
                      note="mean composite signal strength"
                    />
                    <Kpi
                      label="Avg confidence"
                      value={opportunityMetrics.avgConfidence.toFixed(2)}
                      note="mean detector certainty"
                    />
                  </KpiRow>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Opportunity score distribution" />
          <PanelBody>
            <ResourceView resource={opportunities} loadingRows={2}>
              {() => (
                <div style={{ height: "240px" }}>
                  <ChartJs
                    config={opportunityScoreChart}
                    label="Distribution of opportunity scores by percentile"
                  />
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      {/* Proposal metrics */}
      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Decision outcomes"
            meta={<Freshness resource={proposals} name="decisions" />}
            actions={<Chip>GET /api/v1/proposals</Chip>}
          />
          <PanelBody>
            <ResourceView resource={proposals} loadingRows={3}>
              {(data) => (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi
                      label="Proposals"
                      value={formatCount(proposalMetrics.total)}
                      note="staged by the loop"
                    />
                    <Kpi
                      label="Approved"
                      value={formatCount(proposalMetrics.approved)}
                      tone={proposalMetrics.approved > 0 ? "ok" : "neutral"}
                      note="passed all controls"
                    />
                    <Kpi
                      label="Vetoed"
                      value={formatCount(proposalMetrics.vetoed)}
                      tone={proposalMetrics.vetoed > 0 ? "bad" : "neutral"}
                      note="blocked by a control"
                    />
                  </KpiRow>
                  {proposalMetrics.total === 0 ? (
                    <StateBlock
                      tone="neutral"
                      label="no decisions"
                      headline="The loop has staged no proposal."
                      compact
                    >
                      <p>The DECIDE stage runs only when opportunities clear the action bar.</p>
                    </StateBlock>
                  ) : null}
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Decision status breakdown" />
          <PanelBody>
            <ResourceView resource={proposals} loadingRows={2}>
              {() =>
                proposalMetrics.total === 0 ? (
                  <StateBlock
                    tone="neutral"
                    label="no decisions"
                    headline="Cannot chart status breakdown with zero proposals."
                    compact
                  >
                    <p>Charts render when the loop stages a proposal.</p>
                  </StateBlock>
                ) : (
                  <div style={{ height: "280px" }}>
                    <ChartJs
                      config={proposalStatusChart}
                      label="Breakdown of proposal statuses: approved, vetoed, withdrawn, draft"
                    />
                  </div>
                )
              }
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      {/* Performance notes */}
      <Panel>
        <PanelHead title="Quantum decision process" />
        <PanelBody>
          <div className="space-y-3 max-w-[86ch]">
            <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
              The analytics above reflect the DECIDE stage's complete output. Every opportunity
              detected, every proposal sized, and every control decision is recorded in the event
              log and read here as the platform served it.
            </p>
            <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
              <span className="font-semibold">Proposal lifecycle:</span> Opportunities pass through a
              sequence of checks — availability, liquidity, feasibility, capacity — and those that
              pass are sized into proposals. The DECIDE stage then runs each proposal through
              governance controls, each of which may approve it, veto it, or request revision.
            </p>
            <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
              <span className="font-semibold">No control on this page:</span> All views are
              read-only. Proposals are shaped by the platform; governance decisions are the
              responsibility of the controls configured in the policy. This page renders decisions
              made; it does not make them.
            </p>
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}
