"use client";

import { useState } from "react";
import { Chip, Freshness, Metric, MetricRow, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import {
  EmptyBlock,
  MissingEndpointBlock,
  ResourceView,
  UnavailableBlock,
} from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { Autonomy, Governance, Risk } from "@/lib/api/types";
import { isUnavailable } from "@/lib/api/types";
import { formatCount, formatDecimal, formatPercent } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

const AXES = ["all", "instrument", "sector", "venue", "currency", "cell"] as const;

/**
 * Exposure against limits, the governance findings on the agent roster, and
 * the halt state — with the two things the platform explicitly cannot measure
 * here named rather than left blank.
 */
export default function RiskAndCompliance() {
  const risk = useResource<Risk>(platform.risk, {
    key: "risk",
    label: "GET /risk",
    intervalMs: 10_000,
  });
  const autonomy = useResource<Autonomy>(platform.autonomy, {
    key: "autonomy",
    label: "GET /autonomy",
    intervalMs: 20_000,
  });
  const governance = useResource<Governance>(platform.governance, {
    key: "governance",
    label: "GET /system/governance",
    intervalMs: 30_000,
  });

  const [axis, setAxis] = useState<(typeof AXES)[number]>("all");
  const [breachedOnly, setBreachedOnly] = useState(false);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Kill switch and autonomy"
          meta={<Freshness resource={risk} name="risk" />}
          actions={
            // Posture is on this panel, so the paper label is on it too. The
            // banner above covers the screen; it does not travel with a panel
            // lifted into an incident note, and this panel is the one an
            // operator crops. Live-capable is an alarm here, not a mode.
            autonomy.data === null ? null : (
              <StatusChip
                tone={autonomy.data.live ? "bad" : "ok"}
                label={autonomy.data.live ? "LIVE-CAPABLE" : "PAPER TRADING"}
              />
            )
          }
        />
        <PanelBody>
          <ResourceView resource={risk} loadingRows={2}>
            {(data) => (
              <div className="flex flex-col gap-3">
                <MetricRow>
                  <Metric
                    label="Kill switch"
                    value={data.kill_switch.halted ? "TRIPPED" : "clear"}
                    tone={data.kill_switch.halted ? "bad" : "ok"}
                    hint={data.kill_switch.reason || "no reason recorded"}
                  />
                  <Metric
                    label="Halted scopes"
                    value={formatCount(data.kill_switch.halted_scopes.length)}
                    tone={data.kill_switch.halted_scopes.length > 0 ? "bad" : undefined}
                    hint={data.kill_switch.halted_scopes.join(", ") || "none"}
                  />
                  <Metric
                    label="Clearances"
                    value={formatCount(data.kill_switch.clearances)}
                    hint="recorded operator clearances"
                  />
                  <Metric
                    label="Autonomy"
                    value={autonomy.data?.level ?? "—"}
                    hint={autonomy.data ? `ceiling ${autonomy.data.ceiling}` : "reading /autonomy"}
                  />
                  <Metric
                    label="Live"
                    value={autonomy.data === null ? "—" : autonomy.data.live ? "yes" : "no"}
                    tone={autonomy.data?.live ? "bad" : "ok"}
                    hint="whether autonomy permits live trading"
                  />
                </MetricRow>
                {data.kill_switch.halted ? (
                  <p className="text-[12px] text-[color:var(--color-down)]" role="alert">
                    Tripped by <span className="num">{data.kill_switch.tripped_by || "unknown"}</span>
                    {data.kill_switch.reason ? `: ${data.kill_switch.reason}` : ""}. Use the control
                    in the header to clear it.
                  </p>
                ) : null}
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Exposure against concentration limits"
          meta={<Freshness resource={risk} name="exposure" />}
          actions={
            <>
              <label className="sr-only" htmlFor="axis-filter">
                Filter by axis
              </label>
              <select
                id="axis-filter"
                className="select h-[24px] w-[130px]"
                value={axis}
                onChange={(event) => setAxis(event.target.value as (typeof AXES)[number])}
              >
                {AXES.map((option) => (
                  <option key={option} value={option}>
                    {option === "all" ? "every axis" : option}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="btn"
                aria-pressed={breachedOnly}
                data-variant={breachedOnly ? "danger" : "ghost"}
                onClick={() => setBreachedOnly((value) => !value)}
              >
                Breached only
              </button>
            </>
          }
        />
        <PanelBody flush>
          <ResourceView resource={risk} loadingRows={6}>
            {(data) => {
              if (isUnavailable(data.exposure)) {
                return (
                  <div className="p-3">
                    <UnavailableBlock
                      subject={data.exposure.subject}
                      reason={data.exposure.reason}
                    />
                  </div>
                );
              }
              const rows = data.exposure.buckets
                .filter((bucket) => axis === "all" || bucket.axis === axis)
                .filter((bucket) => !breachedOnly || bucket.breached);
              if (rows.length === 0) {
                return (
                  <div className="p-3">
                    <EmptyBlock
                      headline={
                        breachedOnly
                          ? "No bucket breaches its limit."
                          : `No exposure is recorded on the ${axis} axis.`
                      }
                    />
                  </div>
                );
              }
              return (
                <TableWell maxHeight="46vh" label="Exposure buckets">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Axis</th>
                        <th scope="col">Bucket</th>
                        <th scope="col" className="n">
                          Gross
                        </th>
                        <th scope="col" className="n">
                          Net
                        </th>
                        <th scope="col" className="n">
                          Share
                        </th>
                        <th scope="col" className="n">
                          Limit
                        </th>
                        <th scope="col">Utilisation</th>
                      </tr>
                    </thead>
                    <tbody>
                      {rows.map((bucket) => (
                        <tr
                          key={`${bucket.axis}:${bucket.bucket}`}
                          data-alert={bucket.breached ? "true" : undefined}
                        >
                          <td className="num text-[color:var(--color-ink-dim)]">{bucket.axis}</td>
                          <td>{bucket.bucket}</td>
                          <td className="n">{formatDecimal(bucket.gross)}</td>
                          <td className="n" data-direction={directionOfNet(bucket.net)}>
                            {formatDecimal(bucket.net)}
                          </td>
                          <td className="n" data-direction={bucket.breached ? "negative" : undefined}>
                            {formatPercent(bucket.share)}
                          </td>
                          <td className="n text-[color:var(--color-ink-dim)]">
                            {formatPercent(bucket.limit)}
                          </td>
                          <td>
                            <UtilisationBar share={bucket.share} limit={bucket.limit} />
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWell>
              );
            }}
          </ResourceView>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Limit utilisation and tail risk" />
          <PanelBody>
            <ResourceView resource={risk} loadingRows={2}>
              {(data) => (
                <div className="flex flex-col gap-3">
                  <UnavailableBlock
                    subject={data.limit_utilisation.subject}
                    reason={data.limit_utilisation.reason}
                  />
                  <UnavailableBlock subject={data.tail_risk.subject} reason={data.tail_risk.reason} />
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Compliance obligations" actions={<Chip tone="warn">no endpoint</Chip>} />
          <PanelBody>
            <MissingEndpointBlock endpoint={NOT_YET_SERVED["compliance"]!} />
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead
          title="Governance findings"
          meta={<Freshness resource={governance} name="governance" />}
          actions={governance.data ? <Chip>{governance.data.agents} agent(s)</Chip> : null}
        />
        <PanelBody flush>
          <ResourceView resource={governance} loadingRows={3}>
            {(data) =>
              data.findings.length === 0 ? (
                <EmptyBlock headline="The agent roster raises no governance finding.">
                  <p>
                    Observed, not assumed: <code className="num">GET /api/v1/system/governance</code>{" "}
                    reviewed {data.agents} agent(s) and returned no finding.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="34vh" label="Governance findings">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Severity</th>
                        <th scope="col">Rule</th>
                        <th scope="col">Detail</th>
                        <th scope="col">Agents</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.findings.map((finding, index) => (
                        <tr
                          key={`${finding.rule}-${index}`}
                          data-alert={finding.severity === "error" ? "true" : undefined}
                        >
                          <td>
                            <StatusChip
                              tone={finding.severity === "error" ? "bad" : "warn"}
                              label={finding.severity}
                            />
                          </td>
                          <td className="num">{finding.rule}</td>
                          <td className="whitespace-normal text-[11.5px]">{finding.detail}</td>
                          <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                            {finding.agents.join(", ") || "—"}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWell>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Autonomy history"
          meta={<Freshness resource={autonomy} name="autonomy" />}
        />
        <PanelBody flush>
          <ResourceView resource={autonomy} loadingRows={3}>
            {(data) =>
              data.history.length === 0 ? (
                <EmptyBlock headline="Autonomy has not changed since this process started." />
              ) : (
                <TableWell maxHeight="30vh" label="Autonomy changes">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">At (ns since epoch)</th>
                        <th scope="col">From</th>
                        <th scope="col">To</th>
                        <th scope="col">Operator</th>
                        <th scope="col">Reason</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.history.map((change, index) => (
                        <tr key={`${change.at}-${index}`}>
                          <td className="num text-[10px]">{change.at}</td>
                          <td className="num">{change.from}</td>
                          <td className="num">{change.to}</td>
                          <td className="num">{change.operator}</td>
                          <td className="whitespace-normal text-[11.5px]">{change.reason}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWell>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

function directionOfNet(net: string): "positive" | "negative" | "flat" {
  if (net.trim().startsWith("-")) return "negative";
  return /[1-9]/.test(net) ? "positive" : "flat";
}

/** Share against limit. Red only past the limit, where red means breach. */
function UtilisationBar({ share, limit }: { share: number; limit: number }) {
  const ratio = limit > 0 ? Math.min(2, share / limit) : 0;
  const width = Math.min(100, ratio * 50);
  const breached = share > limit;
  return (
    <span
      className="relative block h-[8px] w-[110px] border border-[color:var(--color-line-strong)]"
      role="img"
      aria-label={`${formatPercent(share)} of a ${formatPercent(limit)} limit`}
    >
      <span
        className="absolute inset-y-0 left-0"
        style={{
          width: `${width}%`,
          background: breached ? "var(--color-down)" : "var(--color-ink-faint)",
        }}
      />
      <span
        className="absolute inset-y-0 w-px bg-[color:var(--color-ink-dim)]"
        style={{ left: "50%" }}
        aria-hidden="true"
      />
    </span>
  );
}
