"use client";

import { Chip, Freshness, Metric, MetricRow } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { ResourceView, StateBlock } from "@/components/data/States";
import type { HeatCell } from "@/components/viz/primitives";
import { Heatmap } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { Correlation, CorrelationRefusal } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Pairwise correlation over the platform's own tape, as `GET /api/v1/correlation`
 * computes it — or the platform's reason for refusing to.
 *
 * This page used to be a seeded matrix over fictional instruments under a
 * SIMULATED DATA banner. The route now serves the real statistic with
 * everything needed to recompute it: the window in closes and in returns,
 * the minimum below which an instrument is excluded, and the cycle. Two
 * things the page is careful to keep from the body rather than smooth over:
 *
 * * the **alignment**. The tape keeps closes without their instants, so two
 *   series are lined up by position from the most recent close backwards —
 *   not by timestamp. A reader who assumes the latter will read a lag as a
 *   co-movement. The route says so in words and the page repeats them.
 * * a `null` coefficient. It means the statistic is undefined (a series with
 *   no variance), which is a different fact from a measured zero, and the
 *   heatmap draws it as "not measured".
 *
 * Below the minimum, the route answers `available: false` and lists every
 * instrument it has seen with its close count, so the refusal is a fact with
 * evidence and not a blank panel.
 */

interface Pair {
  readonly a: string;
  readonly b: string;
  readonly value: number;
}

function pairsOf(data: Correlation): readonly Pair[] {
  const pairs: Pair[] = [];
  data.instruments.forEach((a, i) => {
    data.instruments.slice(i + 1).forEach((b) => {
      const value = data.matrix[a]?.[b];
      if (typeof value === "number") pairs.push({ a, b, value });
    });
  });
  // Ranked by magnitude for reading; the values are the platform's.
  return pairs.sort((x, y) => Math.abs(y.value) - Math.abs(x.value));
}

function cellsOf(data: Correlation): readonly HeatCell[] {
  return data.instruments.flatMap((row) =>
    data.instruments.map((column) => ({
      row,
      column,
      value: data.matrix[row]?.[column] ?? null,
    })),
  );
}

function Refusal({ reason, body }: { reason: string; body: CorrelationRefusal }) {
  const observed = body.instruments_observed ?? [];
  const excluded = body.excluded ?? [];
  return (
    <div className="flex flex-col gap-3">
      <StateBlock
        tone="warn"
        label="not available"
        headline="The platform declines to estimate a correlation, in its own words."
      >
        <p data-testid="correlation-refusal">{reason}</p>
        <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
          As of cycle {formatCount(body.as_of_cycle ?? null)}; the estimate requires at least{" "}
          {formatCount(body.minimum_closes ?? null)} closes per instrument.
        </p>
      </StateBlock>
      <TableWell maxHeight="320px" label="Instruments the platform has observed, and how many closes each holds">
        <table className="dt">
          <thead>
            <tr>
              <th scope="col">Instrument observed</th>
              <th scope="col" className="n">
                Closes held
              </th>
              <th scope="col" className="n">
                Minimum
              </th>
            </tr>
          </thead>
          <tbody>
            {observed.length === 0 ? (
              <tr>
                <td colSpan={3} className="text-[11px] text-[color:var(--color-ink-faint)]" data-testid="correlation-none-observed">
                  The platform has observed no instrument: the tape is empty. This is the list the
                  route serves, and it is empty.
                </td>
              </tr>
            ) : (
              observed.map((entry) => (
                <tr key={entry.instrument} data-testid="correlation-observed-row">
                  <td className="num">{entry.instrument}</td>
                  <td className="n">{formatCount(entry.closes)}</td>
                  <td className="n">{formatCount(body.minimum_closes ?? null)}</td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </TableWell>
      {excluded.length > 0 ? <ExcludedTable excluded={excluded} /> : null}
    </div>
  );
}

function ExcludedTable({ excluded }: { excluded: Correlation["excluded"] }) {
  return (
    <TableWell maxHeight="240px" label="Instruments excluded from the estimate, with the platform's reason">
      <table className="dt">
        <thead>
          <tr>
            <th scope="col">Excluded</th>
            <th scope="col" className="n">
              Closes
            </th>
            <th scope="col">Reason</th>
          </tr>
        </thead>
        <tbody>
          {excluded.map((entry) => (
            <tr key={entry.instrument} data-testid="correlation-excluded-row">
              <td className="num">{entry.instrument}</td>
              <td className="n">{formatCount(entry.closes)}</td>
              <td className="text-[11px] text-[color:var(--color-ink-dim)]">{entry.reason}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </TableWell>
  );
}

export default function CorrelationPage() {
  const correlation = useResource<Correlation>(platform.correlation, {
    key: "correlation",
    label: "GET /correlation",
    intervalMs: 20_000,
  });

  const outcome = correlation.outcome;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Pairwise correlation of returns"
          meta={<Freshness resource={correlation} name="correlation" />}
          actions={<Chip>GET /api/v1/correlation</Chip>}
        />
        <PanelBody>
          {outcome !== null && outcome.kind === "unavailable" ? (
            <Refusal reason={outcome.reason} body={outcome.body as unknown as CorrelationRefusal} />
          ) : (
            <ResourceView resource={correlation} loadingRows={6}>
              {(data) => {
                const pairs = pairsOf(data);
                return (
                  <div className="flex flex-col gap-3">
                    <StateBlock
                      tone="info"
                      label="how the series are aligned"
                      headline="Aligned by position, not by time."
                      compact
                    >
                      <p data-testid="correlation-alignment">{data.alignment}</p>
                    </StateBlock>
                    <MetricRow>
                      <Metric label="Statistic" value={data.statistic} hint="as the route names it" />
                      <Metric label="As of cycle" value={formatCount(data.as_of_cycle)} />
                      <Metric label="Window" value={`${formatCount(data.window_closes)} closes`} hint={`${formatCount(data.window_returns)} returns`} />
                      <Metric label="Minimum" value={`${formatCount(data.minimum_closes)} closes`} hint="below it an instrument is excluded" />
                      <Metric label="Instruments" value={formatCount(data.instruments.length)} hint="in the matrix" />
                    </MetricRow>
                    <div className="grid grid-cols-1 gap-3 xl:grid-cols-[3fr_2fr]">
                      <div className="flex flex-col gap-2">
                        <Heatmap
                          cells={cellsOf(data)}
                          rows={[...data.instruments]}
                          columns={[...data.instruments]}
                          label="Pearson correlation of simple returns, as the platform computed it"
                        />
                        <p className="max-w-[80ch] text-[11px] leading-relaxed text-[color:var(--color-ink-dim)]">
                          <span className="text-[color:var(--color-up)]">Green</span> is positive,{" "}
                          <span className="text-[color:var(--color-down)]">red</span> negative, intensity
                          is strength. A dashed cell is a pair the platform could not measure —
                          listed below with its reason — and is not a zero.
                        </p>
                      </div>
                      <TableWell maxHeight="380px" label="Pairs ranked by magnitude">
                        <table className="dt">
                          <thead>
                            <tr>
                              <th scope="col">Pair</th>
                              <th scope="col" className="n">
                                Correlation
                              </th>
                            </tr>
                          </thead>
                          <tbody>
                            {pairs.length === 0 ? (
                              <tr>
                                <td colSpan={2} className="text-[11px] text-[color:var(--color-ink-faint)]">
                                  No pair has a defined coefficient.
                                </td>
                              </tr>
                            ) : (
                              pairs.map((pair) => (
                                <tr key={`${pair.a}:${pair.b}`} data-testid="correlation-pair-row">
                                  <td className="num">
                                    {pair.a} × {pair.b}
                                  </td>
                                  <td
                                    className="n"
                                    data-direction={pair.value > 0 ? "positive" : pair.value < 0 ? "negative" : "flat"}
                                  >
                                    {pair.value.toFixed(2)}
                                  </td>
                                </tr>
                              ))
                            )}
                          </tbody>
                        </table>
                      </TableWell>
                    </div>
                    {data.excluded.length > 0 ? <ExcludedTable excluded={data.excluded} /> : null}
                    {data.undefined.length > 0 ? (
                      <TableWell maxHeight="240px" label="Pairs whose coefficient is undefined">
                        <table className="dt">
                          <thead>
                            <tr>
                              <th scope="col">Pair</th>
                              <th scope="col">Why undefined</th>
                            </tr>
                          </thead>
                          <tbody>
                            {data.undefined.map((pair) => (
                              <tr key={`${pair.a}:${pair.b}`} data-testid="correlation-undefined-row">
                                <td className="num">
                                  {pair.a} × {pair.b}
                                </td>
                                <td className="text-[11px] text-[color:var(--color-ink-dim)]">{pair.reason}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </TableWell>
                    ) : null}
                  </div>
                );
              }}
            </ResourceView>
          )}
        </PanelBody>
      </Panel>
    </div>
  );
}
