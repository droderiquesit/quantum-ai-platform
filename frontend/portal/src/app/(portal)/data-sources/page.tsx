"use client";

import Link from "next/link";
import { useMemo } from "react";
import { Chip, FEED_LABEL, FEED_TONE, Freshness, StatusChip, StreamControls } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, MissingEndpointBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { MeshStatus, Regions } from "@/lib/api/types";
import { formatAgo, formatCount, formatDecimal, formatDurationMs } from "@/lib/format";
import { connections, useConnections } from "@/lib/hooks/connections";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useNow } from "@/lib/hooks/useNow";
import { useRegistrations, type RegistrationSource } from "@/lib/hooks/useRegistrations";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Where the platform's data comes from and whether it can be trusted right now.
 *
 * The platform serves no source registry in this deployment and no per-source
 * health document at all, so both are declared missing by name. What can be
 * measured is measured: the freshness of every edge cell that reports here,
 * what the mesh has absorbed, and the observed latency of each live stream this
 * browser holds open — which is a real provenance record for the only sources
 * this console actually reads.
 *
 * One catalogue the platform does serve: `GET /registrations`, the registration
 * standing of every catalogued source. It is shown here as a badge per source —
 * keyless, registered by a named person, or pending and naming who must
 * register — because a feed catalogue that showed only freshness would let a
 * source read as merely quiet when in fact the platform refuses it until
 * somebody registers. Nothing on this page changes a standing: the badge links
 * to `/data-sources/registrations`, where the one approval an operator records
 * lives, and that approval needs the person.
 */
export default function DataSources() {
  const registry = useResource<unknown>(platform.dataSources, {
    key: "data-sources",
    label: "GET /data-sources",
    intervalMs: 60_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "regions",
    label: "GET /regions",
    intervalMs: 15_000,
  });
  const mesh = useResource<MeshStatus>(platform.mesh, {
    key: "mesh",
    label: "GET /mesh",
    intervalMs: 15_000,
  });
  const venues = useRegistrations();
  const health = useEventStream({ channel: "health", label: "SSE /stream/health", maxEvents: 60 });

  const feeds = useConnections();
  const now = useNow();

  const observedLatency = useMemo(() => {
    const ingest = health.events.map((event) => event.ingestLagMs).filter((v): v is number => v !== null);
    const transit = health.events
      .map((event) => event.transitLagMs)
      .filter((v): v is number => v !== null);
    return {
      ingest: ingest.length === 0 ? null : Math.round(ingest.reduce((a, b) => a + b, 0) / ingest.length),
      transit:
        transit.length === 0 ? null : Math.round(transit.reduce((a, b) => a + b, 0) / transit.length),
    };
  }, [health.events]);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Source registry"
          meta={<Freshness resource={registry} name="data source registry" />}
          actions={<Chip>GET /api/v1/data-sources</Chip>}
        />
        <PanelBody>
          <ResourceView resource={registry} loadingRows={4}>
            {(data) => (
              <EmptyBlock headline="The platform returned a registry body this console does not model.">
                <pre className="num mt-2 max-h-[220px] overflow-auto whitespace-pre-wrap break-all">
                  {JSON.stringify(data, null, 2)}
                </pre>
              </EmptyBlock>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel data-testid="data-sources-registrations">
        <PanelHead
          title="Registration standing per catalogued source"
          meta={<Freshness resource={venues} name="venue registrations" />}
          actions={
            <>
              {venues.data === null ? null : (
                <Chip
                  tone={venues.data.posture === "PAPER TRADING" ? "ok" : "bad"}
                  title="GET /registrations: posture"
                >
                  <span data-testid="data-sources-registrations-posture">{venues.data.posture}</span>
                </Chip>
              )}
              <Chip>GET /api/v1/registrations</Chip>
              <Link className="btn" data-variant="ghost" href={REGISTRATIONS_HREF}>
                Venue registrations
              </Link>
            </>
          }
        />
        <PanelBody flush>
          <p className="border-b border-[color:var(--color-line)] px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            The registry&rsquo;s own answer, read-only here — the same registry the feed&rsquo;s
            admission gate consults, so this page and the gate cannot disagree about who registered.
            A source shown pending is one the platform refuses to read until a named operator records
            the registration on{" "}
            <Link href={REGISTRATIONS_HREF} className="underline">
              venue registrations
            </Link>
            .
          </p>
          <ResourceView resource={venues} loadingRows={3}>
            {(data) =>
              data.sources.length === 0 ? (
                <EmptyBlock headline="The platform catalogues no source's registration requirement.">
                  <p>
                    <code className="num">GET /api/v1/registrations</code> answered with an empty
                    catalogue. Nothing is listed because the platform listed nothing; an absent
                    requirement is a question nobody asked, not a source that needs no account.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="34vh" label="Catalogued sources and their registration standing">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Source</th>
                        <th scope="col">Venue requires</th>
                        <th scope="col">Registration</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.sources.map((source) => (
                        <tr
                          key={source.source_id}
                          data-standing={source.standing.standing}
                          data-alert={source.standing.standing === "pending" ? "true" : undefined}
                        >
                          <td className="num">{source.source_id}</td>
                          <td className="num text-[color:var(--color-ink-dim)]">
                            {source.requirement ?? "not declared"}
                          </td>
                          <td>
                            <RegistrationBadge source={source} />
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
        <PanelHead title="Per-source latency, freshness, quality and provenance" />
        <PanelBody>
          <MissingEndpointBlock endpoint={NOT_YET_SERVED["dataSourceHealth"]!} />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Observed feed health"
          meta={
            <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
              measured in this browser, not reported by the platform
            </span>
          }
          actions={
            <button
              type="button"
              className="btn"
              data-variant="ghost"
              onClick={() => connections.reconnectAll()}
            >
              Reconnect all
            </button>
          }
        />
        <PanelBody flush>
          <div className="grid grid-cols-2 gap-x-6 gap-y-1 border-b border-[color:var(--color-line)] px-3 py-2 text-[11px] sm:grid-cols-4">
            <Stat label="Health stream" value={FEED_LABEL[health.state]} />
            <Stat label="Events seen" value={formatCount(health.received)} />
            <Stat label="Mean ingest lag" value={formatDurationMs(observedLatency.ingest)} />
            <Stat label="Mean transit" value={formatDurationMs(observedLatency.transit)} />
          </div>
          <TableWell maxHeight="30vh" label="Feeds registered on this page">
            <table className="dt">
              <thead>
                <tr>
                  <th scope="col">Source</th>
                  <th scope="col">Transport</th>
                  <th scope="col">State</th>
                  <th scope="col">Last data</th>
                  <th scope="col" className="n">
                    Failed attempts
                  </th>
                  <th scope="col">Provenance</th>
                  <th scope="col" />
                </tr>
              </thead>
              <tbody>
                {feeds.map((feed) => (
                  <tr key={feed.id} data-alert={feed.state === "error" ? "true" : undefined}>
                    <td className="num">{feed.label}</td>
                    <td>
                      <Chip>{feed.kind === "stream" ? "SSE" : "REST"}</Chip>
                    </td>
                    <td>
                      <StatusChip tone={FEED_TONE[feed.state]} label={FEED_LABEL[feed.state]} />
                    </td>
                    <td className="num text-[color:var(--color-ink-dim)]">
                      {feed.lastEventAt === null || now === null
                        ? "no data yet"
                        : formatAgo(feed.lastEventAt, now)}
                    </td>
                    <td className="n">{feed.attempts}</td>
                    <td className="text-[11px] text-[color:var(--color-ink-faint)]">
                      qip-api via /api/gateway
                    </td>
                    <td>
                      <button
                        type="button"
                        className="btn"
                        data-variant="ghost"
                        onClick={() => connections.reconnect(feed.id)}
                        aria-label={`Reconnect ${feed.label}`}
                      >
                        Reconnect
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWell>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Edge cell freshness"
            meta={<Freshness resource={regions} name="edge cells" />}
            actions={
              regions.data ? <Chip>bound {regions.data.freshness_bound}</Chip> : null
            }
          />
          <PanelBody flush>
            <ResourceView resource={regions} loadingRows={4}>
              {(data) =>
                data.cells.length === 0 ? (
                  <EmptyBlock headline="No cell has reported." />
                ) : (
                  <TableWell maxHeight="34vh" label="Edge cells">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Cell</th>
                          <th scope="col">Reported</th>
                          <th scope="col">Age</th>
                          <th scope="col" className="n">
                            Positions
                          </th>
                          <th scope="col" className="n">
                            Gross
                          </th>
                          <th scope="col" className="n">
                            Net
                          </th>
                          <th scope="col">Quality</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.cells.map((cell) => (
                          <tr
                            key={cell.cell}
                            data-alert={
                              cell.stale || cell.reconciliation_breaks > 0 ? "true" : undefined
                            }
                          >
                            <td className="num">{cell.cell}</td>
                            <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                              {cell.reported_at}
                            </td>
                            <td className="num">{cell.age}</td>
                            <td className="n">{formatCount(cell.positions)}</td>
                            <td className="n">{formatDecimal(cell.gross)}</td>
                            <td className="n">{formatDecimal(cell.net)}</td>
                            <td className="flex gap-1">
                              {cell.stale ? <Chip tone="warn">stale</Chip> : <Chip tone="ok">fresh</Chip>}
                              {cell.halted ? <Chip tone="bad">halted</Chip> : null}
                              {cell.reconciliation_breaks > 0 ? (
                                <Chip tone="bad">{cell.reconciliation_breaks} break(s)</Chip>
                              ) : null}
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
            title="Mesh ingest"
            meta={<Freshness resource={mesh} name="mesh" />}
            actions={<StreamControls stream={health} name="health" />}
          />
          <PanelBody>
            <ResourceView resource={mesh} loadingRows={3}>
              {(data) =>
                data.served ? (
                  <dl className="flex flex-col text-[12px]">
                    <Row label="Cells served" value={formatCount(data.cells_served)} />
                    <Row label="Deltas absorbed" value={formatCount(data.deltas_absorbed)} />
                    <Row label="Envelopes dispatched" value={formatCount(data.envelopes_dispatched)} />
                    <Row label="Inbox depth" value={formatCount(data.inbox_depth)} />
                    {data.error ? (
                      <p className="pt-2 text-[11.5px] text-[color:var(--color-down)]">{data.error}</p>
                    ) : null}
                  </dl>
                ) : (
                  <EmptyBlock headline="This process serves no mesh backbone.">
                    <p>
                      <code className="num">GET /api/v1/mesh</code> reports{" "}
                      <span className="num">served: false</span>. Cell deltas have nowhere to land
                      here, so the edge-cell table above will stay empty in this deployment.
                    </p>
                  </EmptyBlock>
                )
              }
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}

/** The page that holds the registration record and the one approval that writes it. */
const REGISTRATIONS_HREF = "/data-sources/registrations";

/**
 * One source's standing, as the platform tagged it, and a way to the page that
 * explains it.
 *
 * A link and never a control: this console records no registration from a
 * catalogue row, and a badge that could be clicked into an approval would be a
 * second write for a fact only a person can make true. The pending badge names
 * the deployment's configured owner rather than saying "pending" alone, because
 * "who has to do something" is the question a refused feed raises.
 */
function RegistrationBadge({ source }: { source: RegistrationSource }) {
  const { standing } = source;
  const kind = standing.standing;
  const tone = kind === "registered" ? "ok" : kind === "keyless" ? "neutral" : "warn";
  const label =
    kind === "keyless"
      ? "keyless"
      : kind === "registered"
        ? `registered by ${standing.operator}`
        : `pending — ${standing.who_must_register} must register`;
  const title =
    kind === "keyless"
      ? "A public endpoint: the platform reads it with no account and no credential."
      : kind === "registered"
        ? // The variable the record names used to be quoted here. It rode on
          // GET /registrations, which is Role::Viewer, and the platform moved
          // it onto GET /registrations/slots at Role::Operator because a slot
          // name is a fact about this deployment's secret store rather than
          // about a venue. This index does not read that route: a tooltip is
          // not worth a second request, and naming a variable this console was
          // no longer served would have been a string invented to fill a
          // sentence.
          `Registered by ${standing.operator}. The deployment variable the credential is read under is on the venue registrations page, from an operator-role route; no credential value is anywhere in this console.`
        : standing.reason;

  return (
    <Link
      href={REGISTRATIONS_HREF}
      className="chip"
      data-tone={tone === "neutral" ? undefined : tone}
      data-standing={kind}
      data-testid={`data-sources-standing-${source.source_id}`}
      title={title}
    >
      {label}
    </Link>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-[color:var(--color-ink-dim)]">{label}</span>
      <span className="num">{value}</span>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-[color:var(--color-line)] py-1 last:border-b-0">
      <dt className="text-[11px] text-[color:var(--color-ink-dim)]">{label}</dt>
      <dd className="num">{value}</dd>
    </div>
  );
}
