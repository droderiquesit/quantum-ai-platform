"use client";

import { useMemo, useState } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Bars } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { Agents, Governance } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The roster, what each agent may read, and what governance makes of it.
 *
 * A capability here is a grant, and the review beside it is the platform's own
 * check on those grants — not this console's opinion of them. The two are read
 * from separate endpoints and shown together because a roster without its
 * findings invites the reader to conclude the grants are fine.
 */
export default function AgentsPage() {
  const agents = useResource<Agents>(platform.agents, {
    key: "agents",
    label: "GET /agents",
    intervalMs: 30_000,
  });
  const governance = useResource<Governance>(platform.governance, {
    key: "governance",
    label: "GET /system/governance",
    intervalMs: 30_000,
  });

  const [selected, setSelected] = useState<string | null>(null);

  const roster = useMemo(() => agents.data?.agents ?? [], [agents.data]);
  const findings = governance.data?.findings ?? [];
  const errors = findings.filter((finding) => finding.severity === "error");

  const byRole = new Map<string, number>();
  const byCapability = new Map<string, number>();
  for (const agent of roster) {
    byRole.set(agent.role, (byRole.get(agent.role) ?? 0) + 1);
    for (const capability of agent.capabilities) {
      byCapability.set(capability, (byCapability.get(capability) ?? 0) + 1);
    }
  }

  const roles = [...byRole.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([label, value]) => ({ label, value, tone: "accent" as const }));
  const capabilities = [...byCapability.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([label, value]) => ({ label, value, tone: "accent" as const }));

  // Which agents a finding names, so a row can be traced to the roster.
  const flagged = new Set(findings.flatMap((finding) => finding.agents));
  const detail = roster.find((agent) => agent.id === selected) ?? null;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Agent roster"
          meta={<Freshness resource={agents} name="agents" />}
          actions={
            <Chip tone={errors.length > 0 ? "bad" : findings.length > 0 ? "warn" : "ok"}>
              {findings.length === 0
                ? "governance clean"
                : `${findings.length} finding(s), ${errors.length} error(s)`}
            </Chip>
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi label="Agents" value={formatCount(roster.length)} note="declared by manifest" />
            <Kpi label="Roles" value={formatCount(roles.length)} note="distinct on the roster" />
            <Kpi
              label="Distinct capabilities granted"
              value={formatCount(capabilities.length)}
              note="the union across every manifest"
            />
            <Kpi
              label="Agents named in a finding"
              value={formatCount(flagged.size)}
              tone={flagged.size > 0 ? "warn" : "ok"}
              note="by the platform's own review"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="By role" />
          <PanelBody>
            {roles.length === 0 ? <EmptyBlock headline="No agent is registered." /> : <Bars items={roles} />}
          </PanelBody>
        </Panel>
        <Panel>
          <PanelHead title="Capability grants" />
          <PanelBody>
            {capabilities.length === 0 ? (
              <EmptyBlock headline="No capability is granted." />
            ) : (
              <Bars items={capabilities} unit=" agents" />
            )}
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead
          title="Governance review"
          meta={<Freshness resource={governance} name="governance" />}
        />
        <PanelBody flush>
          <ResourceView resource={governance} loadingRows={3}>
            {(data) =>
              data.findings.length === 0 ? (
                <EmptyBlock headline={`No finding against ${formatCount(data.agents)} agent(s).`}>
                  <p>
                    The platform reviewed the roster and returned nothing. That is a check that ran
                    and passed, not a check that is absent.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="300px" label="Governance findings">
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
                      {data.findings.map((finding) => (
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

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[3fr_2fr]">
        <Panel>
          <PanelHead title="Manifests" meta={<Freshness resource={agents} name="roster" />} />
          <PanelBody flush>
            <ResourceView resource={agents} loadingRows={8}>
              {(data) =>
                data.agents.length === 0 ? (
                  <EmptyBlock headline="The roster is empty." />
                ) : (
                  <TableWell maxHeight="460px" label="Agent manifests">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Agent</th>
                          <th scope="col">Role</th>
                          <th scope="col">Owner</th>
                          <th scope="col" className="n">
                            Grants
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.agents.map((agent) => (
                          <tr
                            key={agent.id}
                            data-alert={flagged.has(agent.id) ? "true" : undefined}
                            aria-selected={agent.id === selected}
                          >
                            <td>
                              <button
                                type="button"
                                className="text-left underline decoration-dotted underline-offset-2"
                                onClick={() =>
                                  setSelected((current) => (current === agent.id ? null : agent.id))
                                }
                              >
                                {agent.name}
                              </button>
                              <span className="num block text-[10px] text-[color:var(--color-ink-faint)]">
                                {agent.id}
                              </span>
                            </td>
                            <td className="num">{agent.role}</td>
                            <td className="num text-[10.5px]">{agent.owner}</td>
                            <td className="n">{formatCount(agent.capabilities.length)}</td>
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
            title={detail === null ? "Select an agent" : detail.name}
            actions={
              detail === null ? null : (
                <button type="button" className="btn" data-variant="ghost" onClick={() => setSelected(null)}>
                  Clear
                </button>
              )
            }
          />
          <PanelBody>
            {detail === null ? (
              <p className="text-[12px] text-[color:var(--color-ink-faint)]">
                Choose an agent from the roster to read its purpose and the exact capabilities its
                manifest grants it.
              </p>
            ) : (
              <div className="flex flex-col gap-2">
                <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
                  {detail.purpose}
                </p>
                <div className="flex flex-wrap gap-1.5">
                  {detail.capabilities.map((capability) => (
                    <Chip key={capability} tone="info">
                      {capability}
                    </Chip>
                  ))}
                </div>
                {findings
                  .filter((finding) => finding.agents.includes(detail.id))
                  .map((finding) => (
                    <p
                      key={finding.rule}
                      className="text-[11.5px] leading-relaxed text-[color:var(--color-warn)]"
                    >
                      {finding.rule}: {finding.detail}
                    </p>
                  ))}
              </div>
            )}
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
