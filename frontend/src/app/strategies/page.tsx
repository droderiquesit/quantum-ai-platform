"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Bars } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { Capital, Strategies, StrategyCandidate } from "@/lib/api/types";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The ladder: every candidate, the rung it stands on, and whether it holds
 * capital.
 *
 * The rungs are the promotion gate's, in order, and a candidate's stage is the
 * evidence it has cleared — not a label someone applied. The one column that
 * matters most is `holds_capital`: a candidate on a low rung that holds capital
 * would mean the gate was bypassed, so it is rendered as an alert rather than
 * as a value.
 */

/**
 * The gate's stages, weakest evidence first.
 *
 * Kept here rather than read from the platform because the platform serves a
 * candidate's stage, not the ladder. A stage the platform sends that is not in
 * this list is still rendered — as itself, at the end — so a rung added
 * upstream shows up as an unplaced rung rather than disappearing.
 */
const LADDER = ["proposed", "backtested", "paper", "shadow", "live_capped", "retired"] as const;

function rungIndex(stage: string): number {
  const index = LADDER.indexOf(stage as (typeof LADDER)[number]);
  return index === -1 ? LADDER.length : index;
}

export default function StrategiesPage() {
  const strategies = useResource<Strategies>(platform.strategies, {
    key: "strategies",
    label: "GET /strategies",
    intervalMs: 20_000,
  });
  const capital = useResource<Capital>(platform.capital, {
    key: "strategies-capital",
    label: "GET /capital",
    intervalMs: 20_000,
  });

  const candidates: readonly StrategyCandidate[] = strategies.data?.strategies ?? [];
  const holding = candidates.filter((candidate) => candidate.holds_capital);

  const byStage = new Map<string, number>();
  for (const candidate of candidates) {
    byStage.set(candidate.stage, (byStage.get(candidate.stage) ?? 0) + 1);
  }
  const rungs = [...byStage.entries()]
    .sort((a, b) => rungIndex(a[0]) - rungIndex(b[0]))
    .map(([label, value]) => ({
      label,
      value,
      tone: label === "retired" ? ("flat" as const) : ("accent" as const),
    }));

  // A grant is only meaningful against a strategy that exists. Envelopes naming
  // a strategy the ladder does not list are shown separately: that is capital
  // committed to something this console cannot account for.
  const known = new Set(candidates.map((candidate) => candidate.id));
  const unaccounted = (capital.data?.envelopes ?? []).filter(
    (envelope) => !known.has(envelope.strategy),
  );

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Strategy ladder"
          meta={<Freshness resource={strategies} name="strategies" />}
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Candidates"
              value={formatCount(candidates.length)}
              note="registered with the promotion gate"
            />
            <Kpi
              label="Holding capital"
              value={formatCount(holding.length)}
              tone={holding.length > 0 ? "info" : "neutral"}
              note="allocated, not merely promoted"
            />
            <Kpi
              label="Rungs occupied"
              value={formatCount(rungs.length)}
              note={`of ${LADDER.length} the gate defines`}
            />
            <Kpi
              label="Envelopes without a candidate"
              value={formatCount(unaccounted.length)}
              tone={unaccounted.length > 0 ? "bad" : "ok"}
              note="capital committed to a strategy not on the ladder"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[1fr_2fr]">
        <Panel>
          <PanelHead title="Population by rung" />
          <PanelBody>
            {rungs.length === 0 ? (
              <EmptyBlock headline="No candidate is registered." />
            ) : (
              <div className="flex flex-col gap-3">
                <Bars items={rungs} />
                <ol className="flex flex-col gap-1 border-t border-[color:var(--color-line)] pt-2">
                  {LADDER.map((rung, index) => (
                    <li
                      key={rung}
                      className="flex items-center justify-between gap-2 text-[11px] text-[color:var(--color-ink-dim)]"
                    >
                      <span className="num">
                        {index + 1}. {rung}
                      </span>
                      <span className="num">{formatCount(byStage.get(rung) ?? 0)}</span>
                    </li>
                  ))}
                </ol>
              </div>
            )}
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="Candidates"
            meta={<Freshness resource={strategies} name="candidates" />}
          />
          <PanelBody flush>
            <ResourceView resource={strategies} loadingRows={6}>
              {(data) =>
                data.strategies.length === 0 ? (
                  <EmptyBlock headline="The ladder is empty.">
                    <p>
                      <code className="num">GET /api/v1/strategies</code> returned an empty list. No
                      candidate has been registered with the promotion gate in this deployment. The
                      champion/challenger desk that produces them runs in the Deep Brain, which is
                      a separate process and exposes no HTTP surface of its own.
                    </p>
                  </EmptyBlock>
                ) : (
                  <TableWell maxHeight="420px" label="Strategy candidates">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Id</th>
                          <th scope="col">Cell</th>
                          <th scope="col">Venue</th>
                          <th scope="col">Rung</th>
                          <th scope="col">Capital</th>
                          <th scope="col">Registered</th>
                        </tr>
                      </thead>
                      <tbody>
                        {[...data.strategies]
                          .sort(
                            (a, b) => rungIndex(b.stage) - rungIndex(a.stage) || a.id.localeCompare(b.id),
                          )
                          .map((candidate) => (
                            <tr key={candidate.id}>
                              <td className="num">{candidate.id}</td>
                              <td className="num">{candidate.cell}</td>
                              <td className="num">{candidate.venue}</td>
                              <td>
                                <Chip tone={candidate.stage === "retired" ? "neutral" : "info"}>
                                  {candidate.stage}
                                </Chip>
                              </td>
                              <td>
                                <Chip tone={candidate.holds_capital ? "info" : "neutral"}>
                                  {candidate.holds_capital ? "holds" : "none"}
                                </Chip>
                              </td>
                              <td className="num text-[10.5px]">
                                {formatTimestamp(candidate.registered_at)}
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

      {unaccounted.length > 0 ? (
        <Panel>
          <PanelHead
            title="Capital committed to a strategy the ladder does not list"
            meta={<Freshness resource={capital} name="capital" />}
            actions={<Chip tone="bad">{unaccounted.length}</Chip>}
          />
          <PanelBody flush>
            <TableWell maxHeight="240px" label="Unaccounted envelopes">
              <table className="dt">
                <thead>
                  <tr>
                    <th scope="col">Cell</th>
                    <th scope="col">Strategy</th>
                    <th scope="col" className="n">
                      Gross limit
                    </th>
                    <th scope="col">Expires</th>
                  </tr>
                </thead>
                <tbody>
                  {unaccounted.map((envelope) => (
                    <tr key={`${envelope.cell}:${envelope.strategy}`} data-alert="true">
                      <td className="num">{envelope.cell}</td>
                      <td className="num">{envelope.strategy}</td>
                      <td className="n">{formatDecimal(envelope.gross_limit)}</td>
                      <td className="num text-[10.5px]">{formatTimestamp(envelope.expires_at)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableWell>
          </PanelBody>
        </Panel>
      ) : null}
    </div>
  );
}
