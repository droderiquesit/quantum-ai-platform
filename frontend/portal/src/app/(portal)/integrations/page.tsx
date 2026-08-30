"use client";

import { useCallback, useMemo, useState } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { describeOutcome, request, type ApiOutcome } from "@/lib/api/client";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED, STREAM_CHANNELS } from "@/lib/api/endpoints";
import type { OpenApiDocument } from "@/lib/api/types";
import { formatCount, formatDurationMs } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Every route this process serves, and what each one answers right now.
 *
 * The list is read from the platform's own OpenAPI document, which it generates
 * from its route table at the moment of the request. A console that listed
 * routes from a constant in its own source would keep advertising a route the
 * platform had stopped serving, and would be believed.
 *
 * The probe beside each row issues the real request through the same gateway
 * every other page uses, and reports the real answer — including a refusal.
 * Only reads are probed. A POST or DELETE here would be an action taken to
 * populate a table, which is not a thing a status page may do.
 */

/** The versioned prefix the document's paths carry. */
const PREFIX = "/api/v1";

interface Probe {
  readonly outcome: ApiOutcome<unknown>;
  readonly latencyMs: number;
  readonly at: number;
}

interface Route {
  readonly method: string;
  readonly path: string;
  readonly role: string;
  readonly summary: string;
  /** The path relative to the gateway, or null when it cannot be probed. */
  readonly probePath: string | null;
}

function routesOf(document: OpenApiDocument): readonly Route[] {
  const routes: Route[] = [];
  for (const [path, operations] of Object.entries(document.paths)) {
    for (const [method, operation] of Object.entries(operations)) {
      const relative = path.startsWith(PREFIX) ? path.slice(PREFIX.length) : null;
      routes.push({
        method: method.toUpperCase(),
        path,
        role: operation["x-required-role"] ?? "unstated",
        summary: operation.summary ?? operation.operationId ?? "",
        // Streams are long-lived and would never complete a probe; a mutation
        // is not this page's to make.
        probePath:
          method.toLowerCase() === "get" && relative !== null && !relative.startsWith("/stream/")
            ? relative
            : null,
      });
    }
  }
  return routes.sort((a, b) => a.path.localeCompare(b.path) || a.method.localeCompare(b.method));
}

export default function IntegrationsPage() {
  const document = useResource<OpenApiDocument>(platform.openapi, {
    key: "openapi",
    label: "GET /openapi.json",
    intervalMs: 60_000,
  });

  const [probes, setProbes] = useState<Readonly<Record<string, Probe>>>({});
  const [running, setRunning] = useState(false);

  const routes = useMemo(
    () => (document.data === null ? [] : routesOf(document.data)),
    [document.data],
  );
  const probeable = useMemo(() => routes.filter((route) => route.probePath !== null), [routes]);

  const probe = useCallback(async (route: Route) => {
    if (route.probePath === null) return;
    const response = await request<unknown>(route.probePath);
    setProbes((previous) => ({
      ...previous,
      [route.path]: {
        outcome: response.outcome,
        latencyMs: response.latencyMs,
        at: response.receivedAt,
      },
    }));
  }, []);

  const probeAll = useCallback(async () => {
    setRunning(true);
    try {
      // Sequential on purpose. The platform rate-limits per credential, and a
      // burst of thirty concurrent reads would make this page's own probe the
      // reason half the rows report 429.
      for (const route of probeable) {
        await probe(route);
      }
    } finally {
      setRunning(false);
    }
  }, [probeable, probe]);

  const answered = Object.values(probes).filter((entry) => entry.outcome.kind === "ok").length;
  const refused = Object.values(probes).filter(
    (entry) => entry.outcome.kind === "denied",
  ).length;
  const absent = Object.values(probes).filter(
    (entry) => entry.outcome.kind === "unavailable",
  ).length;
  const missing = Object.values(NOT_YET_SERVED);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="API surface"
          meta={<Freshness resource={document} name="the route table" />}
          actions={
            <button
              type="button"
              className="btn"
              data-variant="primary"
              onClick={probeAll}
              disabled={running || probeable.length === 0}
            >
              {running ? "Probing…" : `Probe ${probeable.length} read route(s)`}
            </button>
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Routes served"
              value={formatCount(routes.length)}
              note="from the process's own route table"
            />
            <Kpi
              label="Probed"
              value={formatCount(Object.keys(probes).length)}
              note={`${formatCount(probeable.length)} are readable`}
            />
            <Kpi
              label="Answered with data"
              value={formatCount(answered)}
              tone={answered > 0 ? "ok" : "neutral"}
              note="a 200 carrying a body"
            />
            <Kpi
              label="Answered with an absence"
              value={formatCount(absent)}
              tone={absent > 0 ? "warn" : "neutral"}
              note="the route works; the subsystem is not wired"
            />
            <Kpi
              label="Refused this credential"
              value={formatCount(refused)}
              tone={refused > 0 ? "bad" : "neutral"}
              note="401 or 403 — a role, not a fault"
            />
            <Kpi
              label="Stream channels"
              value={formatCount(STREAM_CHANNELS.length)}
              note="long-lived; not probed here"
            />
          </KpiRow>
          {document.data ? (
            <p className="mt-2 text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
              {document.data.info.title} {document.data.info.version} · OpenAPI{" "}
              {document.data.openapi}. {document.data.info.description}
            </p>
          ) : null}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Routes" meta={<Freshness resource={document} name="routes" />} />
        <PanelBody flush>
          <ResourceView resource={document} loadingRows={10}>
            {() =>
              routes.length === 0 ? (
                <EmptyBlock headline="The document declares no path." />
              ) : (
                <TableWell maxHeight="560px" label="Routes served">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Method</th>
                        <th scope="col">Path</th>
                        <th scope="col">Role</th>
                        <th scope="col">Summary</th>
                        <th scope="col">Answers</th>
                        <th scope="col" className="n">
                          Latency
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {routes.map((route) => {
                        const result = probes[route.path];
                        return (
                          <tr key={`${route.method} ${route.path}`}>
                            <td className="num">
                              <Chip tone={route.method === "GET" ? "neutral" : "warn"}>
                                {route.method}
                              </Chip>
                            </td>
                            <td className="num">{route.path}</td>
                            <td className="num text-[10.5px]">{route.role}</td>
                            <td className="whitespace-normal text-[11.5px]">{route.summary}</td>
                            <td>
                              {route.probePath === null ? (
                                <span className="text-[10.5px] text-[color:var(--color-ink-faint)]">
                                  {route.method === "GET" ? "stream — not probed" : "not probed"}
                                </span>
                              ) : result === undefined ? (
                                <button
                                  type="button"
                                  className="btn"
                                  data-variant="ghost"
                                  onClick={() => void probe(route)}
                                >
                                  Probe
                                </button>
                              ) : (
                                <span
                                  className="text-[11px]"
                                  title={describeOutcome(result.outcome)}
                                  style={{
                                    color:
                                      result.outcome.kind === "ok"
                                        ? "var(--color-up)"
                                        : result.outcome.kind === "unavailable"
                                          ? "var(--color-warn)"
                                          : "var(--color-down)",
                                  }}
                                >
                                  {result.outcome.kind === "ok"
                                    ? "data"
                                    : result.outcome.kind === "unavailable"
                                      ? `absent: ${result.outcome.subject}`
                                      : result.outcome.kind}
                                </span>
                              )}
                            </td>
                            <td className="n">
                              {result === undefined ? "—" : formatDurationMs(result.latencyMs)}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </TableWell>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Stream channels" />
          <PanelBody>
            <ul className="flex flex-col gap-1.5">
              {STREAM_CHANNELS.map((channel) => (
                <li key={channel} className="flex items-center justify-between gap-3">
                  <span className="num text-[12px]">
                    GET {PREFIX}/stream/{channel}
                  </span>
                  <Chip>server-sent events</Chip>
                </li>
              ))}
            </ul>
            <p className="mt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
              Each channel resumes from a cursor after a drop and heartbeats every ten seconds, so
              a quiet market and a dead socket are distinguishable. They are not probed here
              because a stream that connects successfully never completes.
            </p>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="What this console needs and the platform does not serve"
            actions={<Chip tone="warn">{missing.length}</Chip>}
          />
          <PanelBody>
            <ul className="flex flex-col gap-2.5">
              {missing.map((endpoint) => (
                <li key={`${endpoint.method} ${endpoint.path}`} className="flex flex-col gap-0.5">
                  <span className="num text-[11.5px] text-[color:var(--color-warn)]">
                    {endpoint.method} {endpoint.path}
                  </span>
                  <span className="text-[11px] leading-relaxed text-[color:var(--color-ink-dim)]">
                    Needed for {endpoint.needed_for}. {endpoint.note}
                  </span>
                </li>
              ))}
            </ul>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
