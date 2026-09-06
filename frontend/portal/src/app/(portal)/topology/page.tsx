"use client";

import Link from "next/link";
import type { ReactNode } from "react";
import { Chip, Freshness, StatusChip, type Tone } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import {
  EmptyBlock,
  MissingEndpointBlock,
  ResourceView,
  StateBlock,
} from "@/components/data/States";
import { describeOutcome, platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import { WIRE_REDACTIONS } from "@/lib/api/redaction";
import type { Agents, MeshStatus, Regions, SystemStatus, SystemView } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource, type Resource } from "@/lib/hooks/useResource";

/**
 * The service dependency graph — assembled in this browser, from four reads,
 * because the platform serves no such document.
 *
 * That sentence is the page. `GET /api/v1/topology` does not exist: the route
 * table in `backend/crates/apps/qip-api/src/routes.rs` declares 47 patterns and
 * none of them is `/topology`, so there is nothing to fetch and nothing to be
 * out of date with. What there is instead is four routes that each evidence a
 * piece of the shape — `GET /system` for the process answering, `GET /mesh` for
 * the backbone between cells and centre, `GET /regions` for the cells that have
 * reported, `GET /agents` for the roster composed into the process — and this
 * page joins them.
 *
 * Assembling is legitimate; asserting is not. So every node and every edge
 * carries the route that evidenced it, the assembly is declared at the top of
 * the page in the same `MissingEndpointBlock` vocabulary every other absent
 * endpoint uses, and the one inference a graph invites — that an edge is a
 * network path — is refused in words: the edges here mean "the centre holds a
 * report from this cell", which is what `/regions` answers, not "these two
 * processes are connected", which no route in this platform states.
 *
 * **What is deliberately not drawn, and what no longer arrives.** `GET /mesh`
 * carries a `cells[].address`, and that address is the cell's base URL on the
 * mesh transport — its identity, taken from `QIP_MESH_PEER`. `GET
 * /system/status` embeds the same status whole, so it carries every cell
 * address too. Both are `Role::Viewer`. It is an internal endpoint and it is
 * not rendered here in any form, not as a node label, not as a tooltip, not as
 * a title attribute.
 *
 * This comment used to end there and add that the gateway strips
 * `QIP_API_BASE_URL` out of its own error bodies "so an upstream address never
 * reaches a browser". That was true of the pixels and false of the wire: the
 * gateway forwarded `/mesh` unmodified, and the body this page fetched carried
 * `"address":"…"` for every served cell, in the network tab, in memory, and in
 * reach of any extension with host permissions. A page asserting a transport
 * property the transport does not have is worse than one that says nothing,
 * because it is read as an assurance — an operator screenshots it into an
 * incident thread on the strength of it.
 *
 * The gateway now applies the field, so the claim is about something. The
 * declared list is `WIRE_REDACTIONS` in `@/lib/api/redaction`, this page
 * renders that list rather than restating it, the replacement is named in the
 * `x-qip-redacted` response header, and `tests/wire.spec.ts` asserts on the
 * response body a browser receives rather than on the DOM — a DOM assertion is
 * what let the old claim ship.
 *
 * There is no control on this page and there is nothing here that could submit
 * an order.
 */
export default function Topology() {
  const system = useResource<SystemView>(platform.system, {
    key: "topology-system",
    label: "GET /system",
    intervalMs: 15_000,
  });
  const mesh = useResource<MeshStatus>(platform.mesh, {
    key: "topology-mesh",
    label: "GET /mesh",
    intervalMs: 15_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "topology-regions",
    label: "GET /regions",
    intervalMs: 15_000,
  });
  const agents = useResource<Agents>(platform.agents, {
    key: "topology-agents",
    label: "GET /agents",
    intervalMs: 60_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <SurfaceHeader system={system.data} meta={<Freshness resource={system} name="topology inputs" />} />

      <Panel data-testid="topology-missing">
        <PanelHead
          title="There is no topology document, and this is what stands in for one"
          actions={<Chip tone="warn">GET /api/v1/topology</Chip>}
        />
        <PanelBody>
          <MissingEndpointBlock endpoint={NOT_YET_SERVED["topology"]!} />
          <p
            className="mt-2 max-w-[90ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
            data-testid="topology-assembly"
          >
            The graph below is assembled in this browser from four routes that are served —{" "}
            <span className="num">GET /system</span>, <span className="num">GET /mesh</span>,{" "}
            <span className="num">GET /regions</span> and <span className="num">GET /agents</span> —
            and it is not a document the platform holds. No process here computes a dependency graph,
            nothing writes one to the event log, and nothing can replay this picture: it is four
            answers joined at read time by a console, and it is exactly as current as the four reads
            beside it. Every node and every edge names the route that evidenced it, so a reader can
            check the join rather than take it.
          </p>
        </PanelBody>
      </Panel>

      <Panel data-testid="topology-graph">
        <PanelHead
          title="The graph, node by node"
          meta={<Freshness resource={system} name="the centre" />}
          actions={<Chip>GET /api/v1/system</Chip>}
        />
        <PanelBody>
          <ResourceView resource={system} loadingRows={5}>
            {(centre) => <Graph centre={centre} mesh={mesh} regions={regions} agents={agents} />}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel data-testid="topology-evidence">
        <PanelHead title="Evidence, read by read" />
        <PanelBody flush>
          <p className="border-b border-[color:var(--color-line)] px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            A graph joined from four reads is only as complete as the four. One that failed leaves a
            hole in the picture, and a hole that is not stated reads as an absence of that part of
            the platform. Each read reports itself here, in the console&rsquo;s own words for what it
            got back.
          </p>
          <TableWell label="the four reads this graph is assembled from">
            <table className="dt">
              <thead>
                <tr>
                  <th scope="col">Read</th>
                  <th scope="col">Contributes</th>
                  <th scope="col">Outcome</th>
                </tr>
              </thead>
              <tbody>
                <EvidenceRow route="/system" contributes="the process answering, its posture and its event chain" resource={system} />
                <EvidenceRow route="/mesh" contributes="whether a backbone is served between cells and centre" resource={mesh} />
                <EvidenceRow route="/regions" contributes="one node per edge cell that has reported" resource={regions} />
                <EvidenceRow route="/agents" contributes="the roster composed into the process" resource={agents} />
              </tbody>
            </table>
          </TableWell>
        </PanelBody>
      </Panel>

      <Panel data-testid="topology-limits">
        <PanelHead title="What this graph cannot show" />
        <PanelBody>
          <div className="flex flex-col gap-3">
            <StateBlock
              tone="warn"
              label="out of view"
              headline="This console reaches one process, so the rest of the platform is absent rather than healthy."
            >
              <p data-testid="topology-out-of-view">
                Everything above is what <span className="num">qip-api</span> answered about itself
                and about what has reported to it. The platform&rsquo;s other binaries — the fast
                brain, the deep brain and the execution node — serve their own health ports, and no
                route in <span className="num">qip-api</span> lists them, so this console cannot say
                whether they are running. They are drawn nowhere. A node greyed out would claim a
                measurement nobody took, which is worse than the gap.
              </p>
            </StateBlock>
            <StateBlock
              tone="neutral"
              label="withheld on purpose"
              headline="No address, no host and no port is rendered for any node, and none reaches this browser to render."
            >
              <p data-testid="topology-withheld">
                <span className="num">GET /mesh</span> carries a{" "}
                <span className="num">cells[].address</span> per cell — the base URL the mesh
                transport identifies that cell by, configured through{" "}
                <span className="num">QIP_MESH_PEER</span> — and{" "}
                <span className="num">GET /system/status</span> embeds that same status whole, so it
                carries them too. Both answer <span className="num">Role::Viewer</span>. Not drawing
                a field was never the same as not receiving it: this panel used to say an upstream
                address never reached a browser while the body behind the page carried one per
                served cell. The gateway replaces the field before the response leaves this
                console&rsquo;s server and names what it replaced in the{" "}
                <span className="num">x-qip-redacted</span> header, so the claim is one an operator
                can check in the same network tab the value used to sit in.
              </p>
              <ul className="mt-2 flex flex-col gap-1" data-testid="topology-redactions">
                {WIRE_REDACTIONS.map((redaction) => (
                  <li
                    key={`${redaction.route} ${redaction.field}`}
                    data-testid="topology-redaction"
                    data-route={redaction.route}
                    data-field={redaction.field}
                  >
                    <span className="num">GET /api/v1{redaction.route}</span>{" "}
                    <span className="num">{redaction.field}</span> — replaced. {redaction.why}
                  </li>
                ))}
              </ul>
            </StateBlock>
            <StateBlock
              tone="neutral"
              label="no control"
              headline="Nothing on this page acts on the platform."
            >
              <p>
                A topology screen invites controls — restart, drain, isolate — and there are none
                here, because no route exists for any of them and a control that implied one would be
                the same defect as implying a live order path. The two operational writes this
                console declares live on{" "}
                <Link href="/system" className="underline">
                  platform health
                </Link>{" "}
                beside the state they change.
              </p>
            </StateBlock>
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * The join itself.
 *
 * Rendered as tiers rather than a canvas: an SVG graph would need a layout, a
 * layout needs weights, and a weight is a number this console would have made
 * up about a platform that gave it none. Tiers say only what the reads say —
 * these cells reported to this centre, through this backbone if one is served.
 */
function Graph({
  centre,
  mesh,
  regions,
  agents,
}: {
  centre: SystemView;
  mesh: Resource<MeshStatus>;
  regions: Resource<Regions>;
  agents: Resource<Agents>;
}) {
  const cells = regions.data?.cells ?? [];
  const roster = agents.data?.agents ?? [];
  const meshServed = mesh.data?.served === true;

  // "Observed empty" is a real state and not a broken read: the centre
  // answered, and it described nothing beyond itself. It is asserted from the
  // three other reads having *landed with a body*, not from their being
  // absent, because a read still in flight and a read the platform refused are
  // both different facts from a platform with one node in it.
  if (
    cells.length === 0 &&
    roster.length === 0 &&
    !meshServed &&
    regions.data !== null &&
    agents.data !== null &&
    mesh.data !== null
  ) {
    return (
      <div data-testid="topology-empty">
        <EmptyBlock headline="The platform described no topology beyond the process that answered.">
          <p>
            <span className="num">GET /system</span> answered, so there is a process:{" "}
            <span className="num">{centre.cycles}</span> cycle(s) run,{" "}
            <span className="num">{centre.events_logged}</span> event(s) logged. The other three
            reads landed and carried nothing to draw — <span className="num">GET /regions</span>{" "}
            listed no cell, <span className="num">GET /mesh</span> reports{" "}
            <span className="num">served: false</span>, and <span className="num">GET /agents</span>{" "}
            listed no agent. That is a platform this console reached and which described one node,
            not a platform it failed to reach and not a credential a route refused. Those two say so
            in their own words and their own colours.
          </p>
        </EmptyBlock>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <Tier
        title="Edge cells — one per cell that has reported to this centre"
        evidence="GET /regions"
        empty={unread(regions, "GET /regions listed no cell, so no cell node is drawn")}
      >
        {cells.map((cell) => (
          <Node
            key={cell.cell}
            id={cell.cell}
            kind="cell"
            evidence="GET /regions"
            alert={cell.stale || cell.halted || cell.reconciliation_breaks > 0}
            facts={[
              cell.halted ? "halted" : "running",
              cell.stale ? `stale (${cell.age})` : `fresh (${cell.age})`,
              `${formatCount(cell.positions)} position(s)`,
              cell.reconciliation_breaks > 0
                ? `${formatCount(cell.reconciliation_breaks)} reconciliation break(s)`
                : "no reconciliation break",
            ]}
          />
        ))}
      </Tier>

      <Edges
        count={cells.length}
        meshServed={meshServed}
        cellsServed={mesh.data?.cells_served ?? null}
      />

      <Tier
        title={meshServed ? "Mesh backbone — served by this process" : "Mesh backbone — not served here"}
        evidence="GET /mesh"
        empty={unread(
          mesh,
          "GET /mesh reports served: false, so this process is not a backbone and cell deltas have nowhere to land in it",
        )}
      >
        {meshServed ? (
          <Node
            id="mesh backbone"
            kind="mesh"
            evidence="GET /mesh"
            alert={false}
            facts={[
              `${formatCount(mesh.data?.cells_served)} cell(s) served`,
              `${formatCount(mesh.data?.deltas_absorbed)} delta(s) absorbed`,
              `${formatCount(mesh.data?.envelopes_dispatched)} envelope(s) dispatched`,
              `inbox depth ${formatCount(mesh.data?.inbox_depth)}`,
            ]}
          />
        ) : null}
      </Tier>

      <Tier title="The centre — the process that answered" evidence="GET /system" empty="">
        <Node
          id="qip-api (this process)"
          kind="centre"
          evidence="GET /system"
          alert={centre.halted || !centre.chain_intact}
          facts={[
            `autonomy ${centre.autonomy}`,
            `ceiling ${centre.ceiling}`,
            centre.live ? "LIVE-CAPABLE" : "not live-capable",
            centre.halted ? "halted" : "running",
            `${formatCount(centre.cycles)} cycle(s)`,
            centre.chain_intact
              ? "event chain intact"
              : `event chain broken at ${centre.chain_broken_at ?? "an unstated index"}`,
          ]}
        />
      </Tier>

      <Tier
        title="Agents — composed into the process above, not separate services"
        evidence="GET /agents"
        empty={unread(agents, "GET /agents listed no agent, so the roster is empty rather than unknown")}
      >
        {roster.map((agent) => (
          <Node
            key={agent.id}
            id={agent.id}
            kind="agent"
            evidence="GET /agents"
            alert={false}
            facts={[agent.role, `owner ${agent.owner}`, `${formatCount(agent.capabilities.length)} capability(ies)`]}
          />
        ))}
      </Tier>

      <p className="max-w-[90ch] text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
        An edge here means &ldquo;the centre holds a report from this cell&rdquo;, which is what{" "}
        <span className="num">GET /regions</span> answers. It is not a network path, not a call
        graph and not a claim that two processes can currently reach one another: no route in this
        platform states any of those, so this page does not draw them. The tiers are a reading
        order, not a measured layout.
      </p>
    </div>
  );
}

/**
 * What a tier says when it has nothing in it.
 *
 * Three different reasons, never one sentence. A read still in flight, a read
 * that came back without a body — refused, unreachable, or a stated absence —
 * and a read that answered an empty list are three facts, and a tier that said
 * "none" for all three would be the same defect as an empty panel that does not
 * say why. The second arm points at the evidence table rather than repeating the
 * outcome, so there is one place the answer is written down.
 */
function unread(resource: Resource<unknown>, whenEmpty: string): string {
  if (resource.outcome === null) {
    return "this read has not landed yet, so nothing is drawn here and nothing is claimed absent";
  }
  if (resource.data === null) {
    return `this read answered no body — ${describeOutcome(resource.outcome)}. Nothing is drawn here, and this tier is unknown rather than empty; see the evidence table below.`;
  }
  return whenEmpty;
}

/** The edges, stated rather than drawn, with what evidenced them. */
function Edges({
  count,
  meshServed,
  cellsServed,
}: {
  count: number;
  meshServed: boolean;
  cellsServed: number | null;
}) {
  if (count === 0) {
    return (
      <p className="text-[11px] text-[color:var(--color-ink-faint)]" data-testid="topology-no-edges">
        No edge: no cell has reported, so there is no reporting relationship to state.
      </p>
    );
  }
  return (
    <ul className="flex flex-col gap-1" data-testid="topology-edges">
      <li
        className="text-[11.5px] text-[color:var(--color-ink-dim)]"
        data-testid="topology-edge"
        data-from="cells"
        data-to={meshServed ? "mesh" : "centre"}
      >
        <span className="num">{formatCount(count)}</span> cell(s) → {meshServed ? "mesh backbone" : "the centre"}
        <span className="ml-2 text-[color:var(--color-ink-faint)]">
          evidenced by GET /regions: the centre holds a report from each
          {meshServed && cellsServed !== null
            ? `, and GET /mesh reports ${cellsServed} cell(s) served on the backbone`
            : ""}
        </span>
      </li>
      {meshServed ? (
        <li
          className="text-[11.5px] text-[color:var(--color-ink-dim)]"
          data-testid="topology-edge"
          data-from="mesh"
          data-to="centre"
        >
          mesh backbone → the centre
          <span className="ml-2 text-[color:var(--color-ink-faint)]">
            evidenced by GET /mesh: this process serves the backbone and absorbs its deltas
          </span>
        </li>
      ) : null}
    </ul>
  );
}

function Tier({
  title,
  evidence,
  empty,
  children,
}: {
  title: string;
  evidence: string;
  empty: string;
  children: ReactNode;
}) {
  const populated = Array.isArray(children) ? children.length > 0 : children !== null;
  return (
    <section className="flex flex-col gap-1.5" data-testid="topology-tier" data-evidence={evidence}>
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="eyebrow">{title}</span>
        <span className="num text-[10px] text-[color:var(--color-ink-faint)]">{evidence}</span>
      </div>
      {populated ? (
        <div className="flex flex-wrap gap-2">{children}</div>
      ) : (
        <p className="text-[11px] text-[color:var(--color-ink-faint)]">{empty}</p>
      )}
    </section>
  );
}

/**
 * One node.
 *
 * Its label is an identifier the platform answered — a cell name, an agent id,
 * the name of the process this console talks to — and never an address. Its
 * facts are fields, rendered as they came.
 */
function Node({
  id,
  kind,
  evidence,
  facts,
  alert,
}: {
  id: string;
  kind: "cell" | "mesh" | "centre" | "agent";
  evidence: string;
  facts: readonly string[];
  alert: boolean;
}) {
  return (
    <div
      className="flex min-w-[190px] flex-col gap-1 border border-[color:var(--color-line-strong)] px-2.5 py-2"
      data-testid="topology-node"
      data-node={id}
      data-kind={kind}
      data-evidence={evidence}
      data-alert={alert ? "true" : undefined}
    >
      <span className="num text-[12px] font-semibold">{id}</span>
      <span className="eyebrow">{kind}</span>
      <ul className="flex flex-col gap-0.5 text-[11px] text-[color:var(--color-ink-dim)]">
        {facts.map((fact) => (
          <li key={fact}>{fact}</li>
        ))}
      </ul>
      <span className="num text-[10px] text-[color:var(--color-ink-faint)]">{evidence}</span>
    </div>
  );
}

function EvidenceRow({
  route,
  contributes,
  resource,
}: {
  route: string;
  contributes: string;
  resource: Resource<unknown>;
}) {
  const outcome = resource.outcome;
  const kind = outcome?.kind ?? "loading";
  const tone: Tone =
    kind === "ok" ? "ok" : kind === "unavailable" ? "warn" : kind === "loading" ? "neutral" : "bad";
  return (
    <tr data-testid="topology-evidence-row" data-route={route} data-outcome={kind}>
      <td className="num">GET /api/v1{route}</td>
      <td className="text-[11.5px] text-[color:var(--color-ink-dim)]">{contributes}</td>
      <td>
        <Chip tone={tone}>{kind}</Chip>
        <span className="ml-2 text-[11px] text-[color:var(--color-ink-faint)]">
          {outcome === null ? "no answer has landed yet" : describeOutcome(outcome)}
        </span>
      </td>
    </tr>
  );
}

/**
 * The posture declaration, on the page rather than only in the chrome.
 *
 * `GET /system` carries the autonomy level and the ceiling this process is
 * running under, and the graph renders both on the centre node. A posture shown
 * without the `PAPER TRADING` label is the failure `/risk` shipped once, so the
 * label is here unconditionally and the platform's own live capability is
 * reported beside it — as a report, in red if the platform ever says otherwise.
 */
function SurfaceHeader({ system, meta }: { system: SystemView | null; meta: ReactNode }) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "topology-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="topology-header">
      <PanelHead
        title="Topology"
        meta={meta}
        actions={
          <>
            {system === null ? null : (
              <Chip tone={system.live ? "bad" : "ok"} title="GET /system: autonomy and live">
                <span data-testid="topology-autonomy">autonomy {system.autonomy}</span>
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
        <p
          className="max-w-[92ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
          data-testid="topology-declaration"
        >
          <span className="chip mr-2" data-tone="ok" data-testid="topology-paper-label">
            PAPER TRADING
          </span>
          The platform serves no topology document. This page assembles one from{" "}
          <span className="num">GET /system</span>, <span className="num">GET /mesh</span>,{" "}
          <span className="num">GET /regions</span> and <span className="num">GET /agents</span>, and
          says so on every node. There is no control here, nothing on this page can submit an order,
          and no address, host or port is rendered for anything.
        </p>
      </PanelBody>
    </Panel>
  );
}
