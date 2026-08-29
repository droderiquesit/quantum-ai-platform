"use client";

import Link from "next/link";
import { useMemo } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type {
  Governance,
  Health,
  Orders,
  Regions,
  Risk,
  SystemMetrics,
} from "@/lib/api/types";
import { isUnavailable } from "@/lib/api/types";
import { formatCount, formatPercent } from "@/lib/format";
import type { Resource } from "@/lib/hooks/useResource";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Everything currently demanding attention, derived from real surfaces only.
 *
 * No alerts endpoint exists, so this page computes its rows in the browser
 * from six endpoints the platform does serve — it never invents a condition
 * and never simulates one. The failure this design must prevent is the quiet
 * lie: a console that could not read /risk showing an empty list that reads
 * as "nothing is wrong". So the premise is asserted alongside the conclusion:
 * every feed is accounted for below as answered or as a named blind spot, and
 * "nothing is alerting" is only ever claimed about the feeds that answered.
 */

interface DerivedAlert {
  readonly severity: "critical" | "warning";
  /** The endpoint whose answer produced this row. */
  readonly source: string;
  readonly message: string;
  /** The page that owns the underlying fact. */
  readonly href: string;
}

const SEVERITY_RANK: Record<DerivedAlert["severity"], number> = {
  critical: 0,
  warning: 1,
};

/** True when the platform gave an answer — data, or a stated absence. */
function answered(resource: Resource<unknown>): boolean {
  return (
    resource.outcome !== null &&
    (resource.outcome.kind === "ok" || resource.outcome.kind === "unavailable")
  );
}

export default function AlertsPage() {
  const health = useResource<Health>(platform.health, {
    key: "alerts-health",
    label: "GET /health",
    intervalMs: 10_000,
  });
  const risk = useResource<Risk>(platform.risk, {
    key: "alerts-risk",
    label: "GET /risk",
    intervalMs: 10_000,
  });
  const orders = useResource<Orders>(platform.orders, {
    key: "alerts-orders",
    label: "GET /orders",
    intervalMs: 10_000,
  });
  const governance = useResource<Governance>(platform.governance, {
    key: "alerts-governance",
    label: "GET /system/governance",
    intervalMs: 30_000,
  });
  const metrics = useResource<SystemMetrics>(platform.systemMetrics, {
    key: "alerts-metrics",
    label: "GET /system/metrics",
    intervalMs: 10_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "alerts-regions",
    label: "GET /regions",
    intervalMs: 15_000,
  });

  const feeds: readonly {
    readonly name: string;
    readonly resource: Resource<unknown>;
    readonly contributes: string;
  }[] = [
    { name: "GET /health", resource: health, contributes: "halt state; reconciliation break count" },
    { name: "GET /risk", resource: risk, contributes: "kill switch; concentration findings" },
    { name: "GET /orders", resource: orders, contributes: "each reconciliation break, verbatim" },
    { name: "GET /system/governance", resource: governance, contributes: "roster findings" },
    { name: "GET /system/metrics", resource: metrics, contributes: "the live-fill flag" },
    { name: "GET /regions", resource: regions, contributes: "stale and halted edge cells" },
  ];

  const alerts = useMemo<readonly DerivedAlert[]>(() => {
    const list: DerivedAlert[] = [];

    const h = health.data;
    if (h !== null) {
      if (h.halted) {
        list.push({
          severity: "critical",
          source: "GET /health",
          message: "The platform reports itself halted. Nothing trades until it is cleared.",
          href: "/system",
        });
      }
      if (h.reconciliation_breaks > 0) {
        list.push({
          severity: "critical",
          source: "GET /health",
          message: `${formatCount(h.reconciliation_breaks)} reconciliation break(s) between the book and its record.`,
          href: "/orders",
        });
      }
    }

    const r = risk.data;
    if (r !== null) {
      if (r.kill_switch.halted) {
        list.push({
          severity: "critical",
          source: "GET /risk",
          message: `Kill switch tripped by ${r.kill_switch.tripped_by || "an unrecorded actor"}: ${r.kill_switch.reason || "no reason recorded"}.`,
          href: "/risk",
        });
      }
      if (!isUnavailable(r.concentrations)) {
        for (const finding of r.concentrations.findings) {
          list.push({
            severity: "warning",
            source: "GET /risk",
            message: `Concentration on ${finding.axis} bucket ${finding.bucket}: ${formatPercent(finding.share)} of gross against a ${formatPercent(finding.limit)} limit.`,
            href: "/risk",
          });
        }
      }
    }

    const o = orders.data;
    if (o !== null) {
      for (const breach of o.reconciliation_breaks) {
        list.push({
          severity: "critical",
          source: "GET /orders",
          message: `Reconciliation break: ${breach}`,
          href: "/orders",
        });
      }
    }

    const g = governance.data;
    if (g !== null) {
      for (const finding of g.findings) {
        list.push({
          severity: finding.severity === "error" ? "critical" : "warning",
          source: "GET /system/governance",
          message: `${finding.rule}: ${finding.detail} (${finding.agents.join(", ") || "no agent named"})`,
          href: "/agents",
        });
      }
    }

    const m = metrics.data;
    // On a paper-only platform a live fill is the worst possible fact: it means
    // the boundary every layer exists to hold has been crossed somewhere.
    if (m !== null && m.live_fills) {
      list.push({
        severity: "critical",
        source: "GET /system/metrics",
        message:
          "The platform reports at least one LIVE fill. This deployment is paper-only; treat the boundary as breached until every fill is traced.",
        href: "/execution/fills",
      });
    }

    const cells = regions.data?.cells;
    if (cells !== undefined) {
      for (const cell of cells) {
        if (cell.halted) {
          list.push({
            severity: "critical",
            source: "GET /regions",
            message: `Edge cell ${cell.cell} reports itself halted.`,
            href: "/command/regions",
          });
        } else if (cell.stale) {
          list.push({
            severity: "warning",
            source: "GET /regions",
            message: `Edge cell ${cell.cell} is stale: its last report is ${cell.age} old, outside the ${regions.data?.freshness_bound ?? "freshness"} bound.`,
            href: "/command/regions",
          });
        }
      }
    }

    return list.sort(
      (a, b) =>
        SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity] ||
        a.source.localeCompare(b.source) ||
        a.message.localeCompare(b.message),
    );
  }, [health.data, risk.data, orders.data, governance.data, metrics.data, regions.data]);

  const criticals = alerts.filter((alert) => alert.severity === "critical").length;
  const warnings = alerts.length - criticals;
  const answeredCount = feeds.filter((feed) => answered(feed.resource)).length;
  const blind = feeds.filter((feed) => !answered(feed.resource));
  const anyAnswered = answeredCount > 0;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Alerts & incidents"
          actions={
            criticals > 0 ? (
              <Chip tone="bad">{formatCount(criticals)} critical</Chip>
            ) : anyAnswered && alerts.length === 0 ? (
              <Chip tone={blind.length > 0 ? "warn" : "ok"}>
                {blind.length > 0 ? "quiet, with blind spots" : "nothing alerting"}
              </Chip>
            ) : null
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Critical"
              value={anyAnswered ? formatCount(criticals) : "—"}
              tone={!anyAnswered ? "neutral" : criticals > 0 ? "bad" : "ok"}
              note="halts, breaks, and anything touching the paper boundary"
            />
            <Kpi
              label="Warnings"
              value={anyAnswered ? formatCount(warnings) : "—"}
              tone={!anyAnswered ? "neutral" : warnings > 0 ? "warn" : "ok"}
              note="concentrations, stale cells, governance findings"
            />
            <Kpi
              label="Feeds answered"
              value={`${formatCount(answeredCount)} / ${formatCount(feeds.length)}`}
              tone={answeredCount === feeds.length ? "ok" : answeredCount === 0 ? "bad" : "warn"}
              note="an answer is data or a stated absence"
            />
            <Kpi
              label="Blind spots"
              value={formatCount(blind.length)}
              tone={blind.length > 0 ? "bad" : "ok"}
              note="feeds this console could not read just now"
            />
          </KpiRow>
          <p className="pt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            The platform serves no alerts endpoint. Every row below is derived in this console
            from the six surfaces named underneath, on every poll — nothing here is invented,
            and nothing is simulated.
          </p>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Active alerts" />
        <PanelBody flush>
          {alerts.length === 0 ? (
            !anyAnswered ? (
              <StateBlock
                tone="bad"
                label="blind"
                headline="No feed can be read, so nothing can be said about alerts."
              >
                <p>
                  An empty list from a console that cannot reach the platform is not quiet — it
                  is blind. Every feed below shows why it could not be read; fix the reads before
                  trusting any absence here.
                </p>
              </StateBlock>
            ) : (
              <StateBlock
                tone={blind.length > 0 ? "warn" : "neutral"}
                label="quiet"
                headline={
                  blind.length > 0
                    ? "Nothing that could be read is alerting."
                    : "Nothing is alerting."
                }
              >
                <p>
                  {blind.length > 0
                    ? `That claim covers only the ${formatCount(answeredCount)} feed(s) that answered — ${blind
                        .map((feed) => feed.name)
                        .join(", ")} could not be read, and whatever they would report is unknown.`
                    : "All six feeds answered and none of them reported a condition. This is a measured quiet, not a failure to look."}
                </p>
              </StateBlock>
            )
          ) : (
            <TableWell maxHeight="46vh" label="Derived alerts">
              <table className="dt">
                <thead>
                  <tr>
                    <th scope="col">Severity</th>
                    <th scope="col">Source</th>
                    <th scope="col">Condition</th>
                    <th scope="col">Owner</th>
                  </tr>
                </thead>
                <tbody>
                  {alerts.map((alert) => (
                    <tr
                      key={`${alert.source}:${alert.message}`}
                      data-alert={alert.severity === "critical" ? "true" : undefined}
                    >
                      <td>
                        <Chip tone={alert.severity === "critical" ? "bad" : "warn"}>
                          {alert.severity}
                        </Chip>
                      </td>
                      <td className="num text-[10.5px] text-[color:var(--color-ink-dim)]">
                        {alert.source}
                      </td>
                      <td className="whitespace-normal">{alert.message}</td>
                      <td>
                        <Link
                          href={alert.href}
                          className="underline decoration-dotted underline-offset-2"
                        >
                          {alert.href}
                        </Link>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableWell>
          )}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Feeds and blind spots"
          actions={
            blind.length > 0 ? (
              <Chip tone="bad">{formatCount(blind.length)} unreadable</Chip>
            ) : (
              <Chip tone="ok">all feeds readable</Chip>
            )
          }
        />
        <PanelBody flush>
          <TableWell maxHeight="360px" label="Alert feed status">
            <table className="dt">
              <thead>
                <tr>
                  <th scope="col">Feed</th>
                  <th scope="col">Contributes</th>
                  <th scope="col">Reading</th>
                </tr>
              </thead>
              <tbody>
                {feeds.map((feed) => (
                  <tr key={feed.name} data-alert={!answered(feed.resource) ? "true" : undefined}>
                    <td className="num">{feed.name}</td>
                    <td className="whitespace-normal text-[11.5px] text-[color:var(--color-ink-dim)]">
                      {feed.contributes}
                    </td>
                    <td>
                      <div className="flex items-center gap-2">
                        <Freshness resource={feed.resource} name={feed.name} />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWell>
          <p className="px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            A feed marked absent gave a stated absence — an answer, and one that can carry no
            alert. A feed marked anything worse is a blind spot: this page cannot see what that
            surface would report, and says so rather than counting it as clear.
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}
