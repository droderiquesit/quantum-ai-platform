"use client";

import { useMemo } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Opportunities } from "@/lib/api/types";
import { formatCount, formatPercent } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Multi-leg arbitrage, as the engine reports it — which today is a stated
 * absence, rendered as such.
 *
 * The panel is still written for the day the engine answers with data. Because
 * no committed shape exists for that answer, the table below is generic and
 * deliberately defensive: it renders an `arbitrage` or `paths` array of
 * objects column-for-column as strings, and anything else as an explicit
 * "unmodelled" state. Guessing a schema for an arbitrage engine and rendering
 * the guess as fact is precisely the kind of invented figure this console
 * exists to avoid.
 */
export default function MultiLegArbitragePage() {
  const arbitrage = useResource<unknown>(platform.arbitrage, {
    key: "execution-arbitrage",
    label: "GET /arbitrage",
    intervalMs: 15_000,
  });
  const opportunities = useResource<Opportunities>(platform.opportunities, {
    key: "execution-arbitrage-opportunities",
    label: "GET /opportunities",
    intervalMs: 15_000,
  });

  const queue = useMemo(() => opportunities.data?.opportunities ?? [], [opportunities.data]);

  // A loose match on detector names, because no detector taxonomy is served.
  // The total queue size is shown beside the matched count so a reader can
  // tell "no arbitrage opportunity" from "no opportunity at all".
  const matched = useMemo(
    () =>
      queue.filter((opportunity) =>
        opportunity.detectors.some((detector) => detector.toLowerCase().includes("arb")),
      ),
    [queue],
  );

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Arbitrage engine"
          meta={<Freshness resource={arbitrage} name="arbitrage" />}
          actions={<Chip>GET /api/v1/arbitrage</Chip>}
        />
        <PanelBody>
          <ResourceView resource={arbitrage} loadingRows={4}>
            {(data) => <ArbitrageBody body={data} />}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Arbitrage-flagged opportunities"
          meta={<Freshness resource={opportunities} name="opportunities" />}
          actions={
            opportunities.data ? (
              <Chip>
                {formatCount(matched.length)} of {formatCount(queue.length)} queued
              </Chip>
            ) : null
          }
        />
        <PanelBody flush>
          <ResourceView resource={opportunities} loadingRows={5}>
            {(data) =>
              data.opportunities.length === 0 ? (
                <EmptyBlock headline="The opportunity queue is empty.">
                  <p>
                    Observed, not assumed: <code className="num">GET /api/v1/opportunities</code>{" "}
                    answered with zero entries. Nothing is queued for any detector, arbitrage
                    included.
                  </p>
                </EmptyBlock>
              ) : matched.length === 0 ? (
                <EmptyBlock headline="No queued opportunity names an arbitrage detector.">
                  <p>
                    {formatCount(data.opportunities.length)} opportunit
                    {data.opportunities.length === 1 ? "y is" : "ies are"} queued, and none carries
                    a detector whose name contains &ldquo;arb&rdquo;. The queue is live; the
                    arbitrage slice of it is empty.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="40vh" label="Arbitrage-flagged opportunities">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Id</th>
                        <th scope="col">Headline</th>
                        <th scope="col" className="n">
                          Score
                        </th>
                        <th scope="col" className="n">
                          Confidence
                        </th>
                        <th scope="col">Detectors</th>
                      </tr>
                    </thead>
                    <tbody>
                      {matched.map((opportunity) => (
                        <tr key={opportunity.id}>
                          <td className="num">{opportunity.id}</td>
                          <td className="whitespace-normal">{opportunity.headline}</td>
                          <td className="n">{opportunity.score.toFixed(3)}</td>
                          <td className="n">{formatPercent(opportunity.confidence)}</td>
                          <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                            {opportunity.detectors.join(", ") || "—"}
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

/**
 * Render whatever the arbitrage endpoint answered with data in it.
 *
 * A stated absence never reaches this component — the client classifies
 * `available: false` bodies upstream and ResourceView renders the engine's own
 * reason. So this handles only the data case, against a shape no type models.
 */
function ArbitrageBody({ body }: { body: unknown }) {
  const rows = extractObjectRows(body);

  if (rows === null) {
    return (
      <StateBlock
        tone="info"
        label="unmodelled"
        headline="The platform returned an arbitrage body this console does not model."
      >
        <p>
          <code className="num">GET /api/v1/arbitrage</code> answered with data, but not in the one
          shape this page renders — an <code className="num">arbitrage</code> or{" "}
          <code className="num">paths</code> array of objects. Nothing is rendered from it here,
          because a shape this page has not been written against would have to be guessed at.
        </p>
      </StateBlock>
    );
  }

  const first = rows[0];
  if (first === undefined) {
    return (
      <EmptyBlock headline="The engine reports no arbitrage path.">
        <p>
          The arbitrage body was served and its list is empty — a measured zero from the engine,
          not an unreachable endpoint.
        </p>
      </EmptyBlock>
    );
  }

  // Columns come from the first row, values are rendered as strings, and a key
  // a later row lacks renders as a dash. Nothing numeric is computed from any
  // of it: an unmodelled shape is displayed, never interpreted.
  const columns = Object.keys(first);

  return (
    <TableWell maxHeight="40vh" label="Arbitrage paths">
      <table className="dt">
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column} scope="col">
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={index}>
              {columns.map((column) => (
                <td key={column} className="num whitespace-normal text-[11px]">
                  {cellText(row[column])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </TableWell>
  );
}

/** The `arbitrage` or `paths` array, if the body carries one made of objects. */
function extractObjectRows(body: unknown): readonly Record<string, unknown>[] | null {
  if (typeof body !== "object" || body === null) return null;
  const record = body as Record<string, unknown>;
  for (const key of ["arbitrage", "paths"]) {
    const candidate = record[key];
    if (!Array.isArray(candidate)) continue;
    const allObjects = candidate.every(
      (entry) => typeof entry === "object" && entry !== null && !Array.isArray(entry),
    );
    return allObjects ? (candidate as readonly Record<string, unknown>[]) : null;
  }
  return null;
}

/** One cell of an unmodelled row, shown verbatim rather than interpreted. */
function cellText(value: unknown): string {
  if (value === undefined) return "—";
  if (typeof value === "string") return value;
  return JSON.stringify(value) ?? String(value);
}
