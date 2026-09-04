"use client";

import { Chip, Freshness, Metric, MetricRow, StatusChip, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { RunCycleCard } from "@/components/data/RunCycle";
import { EmptyBlock, MissingEndpointBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { Agents, MeshStatus, Models, Quantum, Regions, SystemView } from "@/lib/api/types";
import { formatCount, formatDecimal, formatMicros } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * What is actually running, and whether its record of itself holds together.
 *
 * The platform serves no topology document, so this page assembles one from the
 * four surfaces that do exist and says as much. The event chain's integrity is
 * given the same prominence as liveness: a process that is up but whose audit
 * log no longer verifies is a worse condition than one that is down.
 */
export default function SystemTopology() {
  const system = useResource<SystemView>(platform.system, {
    key: "system",
    label: "GET /system",
    intervalMs: 10_000,
  });
  const mesh = useResource<MeshStatus>(platform.mesh, {
    key: "system-mesh",
    label: "GET /mesh",
    intervalMs: 10_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "system-regions",
    label: "GET /regions",
    intervalMs: 10_000,
  });
  const agents = useResource<Agents>(platform.agents, {
    key: "agents",
    label: "GET /agents",
    intervalMs: 60_000,
  });
  const models = useResource<Models>(platform.models, {
    key: "models",
    label: "GET /models",
    intervalMs: 60_000,
  });
  const quantum = useResource<Quantum>(platform.quantum, {
    key: "quantum",
    label: "GET /quantum",
    intervalMs: 60_000,
  });
  const health = useEventStream({ channel: "health", label: "SSE /stream/health", maxEvents: 120 });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Process and event chain"
          meta={<Freshness resource={system} name="system" />}
          actions={
            // Posture is on this panel, so the paper label is on it too. The
            // hint beside the autonomy metric carries the lowercase word
            // "paper" inside a sentence, which is a substring and not a label:
            // it read as a declaration to nobody and would satisfy a test
            // matching on "paper" while this panel carried no posture at all.
            system.data === null ? null : (
              <StatusChip
                tone={system.data.live ? "bad" : "ok"}
                label={system.data.live ? "LIVE-CAPABLE" : "PAPER TRADING"}
              />
            )
          }
        />
        <PanelBody>
          <ResourceView resource={system} loadingRows={2}>
            {(data) => (
              <>
                <MetricRow>
                  <Metric
                    label="Halt state"
                    value={data.halted ? "HALTED" : "running"}
                    tone={data.halted ? "bad" : "ok"}
                    hint={data.halted_scopes.join(", ") || "no scope halted"}
                  />
                  <Metric
                    label="Autonomy"
                    value={data.autonomy}
                    hint={`ceiling ${data.ceiling}${data.live ? " · live" : " · paper"}`}
                    tone={data.live ? "bad" : undefined}
                  />
                  <Metric label="Cycles" value={formatCount(data.cycles)} hint="loop iterations" />
                  <Metric
                    label="Events logged"
                    value={formatCount(data.events_logged)}
                    hint="in this process"
                  />
                  <Metric
                    label="Event chain"
                    value={data.chain_intact ? "intact" : "BROKEN"}
                    tone={data.chain_intact ? "ok" : "bad"}
                    hint={
                      data.chain_broken_at === null
                        ? "hash chain verifies end to end"
                        : `first broken link at sequence ${data.chain_broken_at}`
                    }
                  />
                </MetricRow>
                {!data.chain_intact ? (
                  <p className="mt-2 text-[12px] text-[color:var(--color-down)]" role="alert">
                    The event log&rsquo;s hash chain does not verify from sequence{" "}
                    {data.chain_broken_at}. Nothing this process reports about its own history can be
                    relied on past that point.
                  </p>
                ) : null}
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Service topology" actions={<Chip tone="warn">assembled here</Chip>} />
        <PanelBody>
          <MissingEndpointBlock endpoint={NOT_YET_SERVED["topology"]!} />
          <div className="mt-3 grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-4">
            <ServiceCard
              name="qip-api"
              detail="REST and SSE surface"
              tone={system.outcome?.kind === "ok" ? "ok" : "bad"}
              state={system.outcome?.kind === "ok" ? "answering" : "not answering"}
              reads="/system, /health"
            />
            <ServiceCard
              name="intelligence loop"
              detail="opportunity → proposal → order"
              tone={system.data?.halted ? "bad" : system.data ? "ok" : "neutral"}
              state={
                system.data === null
                  ? "unknown"
                  : system.data.halted
                    ? "halted"
                    : `${formatCount(system.data.cycles)} cycles`
              }
              reads="/system"
            />
            <ServiceCard
              name="mesh backbone"
              detail="central half"
              tone={mesh.data?.served ? "ok" : "warn"}
              state={mesh.data === null ? "unknown" : mesh.data.served ? "served" : "not served"}
              reads="/mesh"
            />
            <ServiceCard
              name="edge cells"
              detail="regional books"
              tone={
                regions.outcome?.kind === "unavailable"
                  ? "warn"
                  : (regions.data?.cells.length ?? 0) > 0
                    ? "ok"
                    : "neutral"
              }
              state={
                regions.outcome?.kind === "unavailable"
                  ? "none reporting"
                  : `${formatCount(regions.data?.cells.length)} reporting`
              }
              reads="/regions"
            />
          </div>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Process health stream"
          meta={<StreamControls stream={health} name="health" />}
        />
        <PanelBody flush>
          <EventFeed stream={health} channel="health" maxHeight="30vh" />
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Edge cells" meta={<Freshness resource={regions} name="regions" />} />
          <PanelBody flush>
            <ResourceView resource={regions} loadingRows={3}>
              {(data) =>
                data.cells.length === 0 ? (
                  <EmptyBlock headline="No cell has reported." />
                ) : (
                  <TableWell maxHeight="30vh" label="Edge cells">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Cell</th>
                          <th scope="col">Age</th>
                          <th scope="col" className="n">
                            Strategies
                          </th>
                          <th scope="col" className="n">
                            Gross
                          </th>
                          <th scope="col">State</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.cells.map((cell) => (
                          <tr key={cell.cell} data-alert={cell.stale ? "true" : undefined}>
                            <td className="num">{cell.cell}</td>
                            <td className="num">{cell.age}</td>
                            <td className="n">{formatCount(cell.strategies)}</td>
                            <td className="n">{formatDecimal(cell.gross)}</td>
                            <td className="flex gap-1">
                              <Chip tone={cell.stale ? "warn" : "ok"}>
                                {cell.stale ? "stale" : "fresh"}
                              </Chip>
                              {cell.halted ? <Chip tone="bad">halted</Chip> : null}
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
            title="Agent roster"
            meta={<Freshness resource={agents} name="agents" />}
            actions={agents.data ? <Chip>{agents.data.agents.length} agent(s)</Chip> : null}
          />
          <PanelBody flush>
            <ResourceView resource={agents} loadingRows={4}>
              {(data) =>
                data.agents.length === 0 ? (
                  <EmptyBlock headline="No agent is registered in this organisation." />
                ) : (
                  <TableWell maxHeight="30vh" label="Agents">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Agent</th>
                          <th scope="col">Role</th>
                          <th scope="col">Owner</th>
                          <th scope="col">Capabilities</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.agents.map((agent) => (
                          <tr key={agent.id} title={agent.purpose}>
                            <td>
                              <span className="block text-[12px]">{agent.name}</span>
                              <span className="num block text-[10px] text-[color:var(--color-ink-faint)]">
                                {agent.id}
                              </span>
                            </td>
                            <td className="num">{agent.role}</td>
                            <td className="num text-[color:var(--color-ink-dim)]">{agent.owner}</td>
                            <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                              {agent.capabilities.join(", ") || "none"}
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

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Model spend" meta={<Freshness resource={models} name="models" />} />
          <PanelBody>
            <ResourceView resource={models} loadingRows={3}>
              {(data) => (
                <>
                  <MetricRow>
                    <Metric label="Agent runs" value={formatCount(data.observed_use.agent_runs)} />
                    <Metric label="Model calls" value={formatCount(data.observed_use.model_calls)} />
                    <Metric label="Tokens" value={formatCount(data.observed_use.tokens)} />
                    <Metric
                      label="Cost"
                      value={formatMicros(data.observed_use.cost_micros)}
                      hint="charged in micros"
                    />
                  </MetricRow>
                  <p className="mt-2 text-[11.5px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    {data.registry.reason}
                  </p>
                </>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Quantum routing" meta={<Freshness resource={quantum} name="quantum" />} />
          <PanelBody>
            <ResourceView resource={quantum} loadingRows={3}>
              {(data) => (
                <div className="flex flex-col gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <StatusChip
                      tone={data.routing.provider === "none" ? "neutral" : "info"}
                      label={`provider ${data.routing.provider}`}
                    />
                    <Chip tone="ok">classical baseline {data.routing.classical_baseline}</Chip>
                  </div>
                  <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                    {data.routing.note}
                  </p>
                  <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    {data.jobs.reason}
                  </p>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      <RunCycleCard
        onRan={() => {
          system.refresh();
          mesh.refresh();
          regions.refresh();
        }}
      />
    </div>
  );
}

function ServiceCard({
  name,
  detail,
  state,
  tone,
  reads,
}: {
  name: string;
  detail: string;
  state: string;
  tone: "ok" | "warn" | "bad" | "neutral";
  reads: string;
}) {
  return (
    <div className="flex flex-col gap-1.5 border border-[color:var(--color-line)] bg-[color:var(--color-sunken)] p-2.5">
      <div className="flex items-center gap-2">
        <StatusChip tone={tone} label={state} />
      </div>
      <span className="num text-[12px] text-[color:var(--color-ink)]">{name}</span>
      <span className="text-[11px] text-[color:var(--color-ink-dim)]">{detail}</span>
      <span className="num text-[10px] text-[color:var(--color-ink-faint)]">reads {reads}</span>
    </div>
  );
}
