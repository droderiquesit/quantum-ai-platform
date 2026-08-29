"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Autonomy, Governance } from "@/lib/api/types";
import { formatClock, formatCount, formatUtcDate } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The autonomy level, its ceiling, and every change ever made — as a record.
 *
 * This page is a record, not a control. Autonomy changes only through
 * `AutonomyController::request_change` carrying an authenticated operator
 * identity, and no surface in this console can raise it. The three
 * paper-trading layers — Terraform's plan-time refusal of a live ceiling,
 * `AutonomyLevel::deployable`'s start-up refusal in every composition root,
 * and the `Cell` type having no live constructor — hold regardless of what
 * any UI does. If `live` ever reads true here, that is a defect to report,
 * not a mode this page offers a way into.
 */

/**
 * A change's `at` is epoch nanoseconds as a JSON number (the API writes
 * `Timestamp::as_nanos()`), not an RFC 3339 string, so `formatTimestamp`
 * would mangle it. Converted to milliseconds for the clock formatters.
 */
function formatChangeInstant(atNanos: number): string {
  const millis = atNanos / 1_000_000;
  return `${formatUtcDate(millis)} ${formatClock(millis)}`;
}

export default function AutonomyPage() {
  const autonomy = useResource<Autonomy>(platform.autonomy, {
    key: "autonomy",
    label: "GET /autonomy",
    intervalMs: 15_000,
  });
  const governance = useResource<Governance>(platform.governance, {
    key: "autonomy-governance",
    label: "GET /system/governance",
    intervalMs: 30_000,
  });

  const live = autonomy.data?.live ?? null;
  const findings = governance.data?.findings ?? [];
  const errors = findings.filter((finding) => finding.severity === "error");

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Autonomy posture"
          meta={<Freshness resource={autonomy} name="autonomy" />}
          actions={
            live === null ? null : live ? (
              <Chip tone="bad">LIVE — a defect on this deployment</Chip>
            ) : (
              <Chip tone="ok">PAPER TRADING</Chip>
            )
          }
        />
        <PanelBody>
          <ResourceView resource={autonomy} loadingRows={2}>
            {(a) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Current level"
                    value={a.level}
                    note="what the controller is running at now"
                  />
                  <Kpi
                    label="Ceiling"
                    value={a.ceiling}
                    note="the most any change request could reach"
                  />
                  <Kpi
                    label="Live"
                    value={a.live ? "true" : "false"}
                    tone={a.live ? "bad" : "ok"}
                    note={
                      a.live
                        ? "on this deployment that is a defect — report it"
                        : "paper trading, as deployed and as designed"
                    }
                  />
                  <Kpi
                    label="Changes recorded"
                    value={formatCount(a.history.length)}
                    note="every change the controller has ever accepted"
                  />
                </KpiRow>
                <p className="mt-2 max-w-[90ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                  This page is a record, not a control. Autonomy changes only through{" "}
                  <span className="num">AutonomyController::request_change</span> with an
                  authenticated operator identity, and no surface in this console can raise it.
                  The three paper-trading layers — the Terraform plan-time refusal of a live
                  ceiling, the <span className="num">AutonomyLevel::deployable</span> start-up
                  refusal, and the <span className="num">Cell</span> type having no live
                  constructor — hold regardless of what any UI does.
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Change history"
          meta={<Freshness resource={autonomy} name="the change history" />}
        />
        <PanelBody flush>
          <ResourceView resource={autonomy} loadingRows={4}>
            {(a) =>
              a.history.length === 0 ? (
                <EmptyBlock headline="The controller has never accepted a change.">
                  <p>
                    The level this process started at is the level it still runs at. An empty
                    history is a statement the controller makes, not a gap in this console.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="380px" label="Autonomy change history">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">When (UTC)</th>
                        <th scope="col">Change</th>
                        <th scope="col">Operator</th>
                        <th scope="col">Reason</th>
                      </tr>
                    </thead>
                    <tbody>
                      {a.history.map((change) => (
                        <tr key={`${change.at}:${change.from}:${change.to}`}>
                          <td className="num text-[10.5px]">{formatChangeInstant(change.at)}</td>
                          <td className="num">
                            {change.from} → {change.to}
                          </td>
                          <td className="num text-[10.5px]">{change.operator}</td>
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

      <Panel>
        <PanelHead
          title="Governance review"
          meta={<Freshness resource={governance} name="governance" />}
          actions={
            governance.data === null ? null : (
              <Chip tone={errors.length > 0 ? "bad" : findings.length > 0 ? "warn" : "ok"}>
                {findings.length === 0
                  ? "clean"
                  : `${findings.length} finding(s), ${errors.length} error(s)`}
              </Chip>
            )
          }
        />
        <PanelBody flush>
          <ResourceView resource={governance} loadingRows={3}>
            {(g) =>
              g.findings.length === 0 ? (
                <EmptyBlock headline={`No finding against ${formatCount(g.agents)} agent(s).`}>
                  <p>
                    The platform reviewed the roster and returned nothing. That is a check that
                    ran and passed, not a check that is absent.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="320px" label="Governance findings">
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
                      {g.findings.map((finding) => (
                        <tr
                          key={`${finding.rule}:${finding.agents.join(",")}`}
                          data-alert={finding.severity === "error" ? "true" : undefined}
                        >
                          <td>
                            <Chip tone={finding.severity === "error" ? "bad" : "warn"}>
                              {finding.severity}
                            </Chip>
                          </td>
                          <td className="num">{finding.rule}</td>
                          <td className="whitespace-normal">{finding.detail}</td>
                          <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                            {finding.agents.join(", ")}
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
    </div>
  );
}
