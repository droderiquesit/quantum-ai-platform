"use client";

import { Chip, Freshness, KeyValue, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, UnavailableBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Capital, Risk } from "@/lib/api/types";
import { isUnavailable } from "@/lib/api/types";
import { formatCount, formatDecimal, formatPercent } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Limit utilisation — or, today, the honest shape of its absence.
 *
 * The platform states outright that it cannot serve limit utilisation, and
 * this page's job is to put that statement where a utilisation figure would
 * sit, next to the limits that do exist. A limit whose utilisation cannot be
 * read is a limit nobody can see approaching, and hiding the panel would hide
 * exactly that.
 *
 * No ratio on this page is computed in the browser. Bounds and exposure arrive
 * as decimal strings and are rendered without being parsed, so where the
 * platform does not serve a utilisation figure, none appears — a percentage
 * invented from two money strings would be trading arithmetic done in the one
 * place it is forbidden.
 */
export default function LimitUtilisationPage() {
  const risk = useResource<Risk>(platform.risk, {
    key: "risk-limits",
    label: "GET /risk",
    intervalMs: 10_000,
  });
  const capital = useResource<Capital>(platform.capital, {
    key: "risk-limits-capital",
    label: "GET /capital",
    intervalMs: 15_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Kill switch"
          meta={<Freshness resource={risk} name="kill switch" />}
        />
        <PanelBody>
          <ResourceView resource={risk} loadingRows={2}>
            {(data) => (
              <div className="flex flex-wrap items-center gap-2">
                <StatusChip
                  tone={data.kill_switch.halted ? "bad" : "ok"}
                  label={data.kill_switch.halted ? "TRIPPED" : "clear"}
                  pulse={data.kill_switch.halted}
                  title={data.kill_switch.reason || "no reason recorded"}
                />
                <Chip
                  tone={data.kill_switch.halted_scopes.length > 0 ? "bad" : "neutral"}
                  title={data.kill_switch.halted_scopes.join(", ") || "no scope halted"}
                >
                  {formatCount(data.kill_switch.halted_scopes.length)} halted scope(s)
                </Chip>
                <Chip tone="neutral">
                  {formatCount(data.kill_switch.clearances)} clearance(s) recorded
                </Chip>
                {data.kill_switch.halted ? (
                  <span className="text-[12px] text-[color:var(--color-down)]" role="alert">
                    Tripped by{" "}
                    <span className="num">{data.kill_switch.tripped_by || "unknown"}</span>
                    {data.kill_switch.reason ? `: ${data.kill_switch.reason}` : ""}
                  </span>
                ) : null}
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Exposure buckets"
          meta={<Freshness resource={risk} name="exposure" />}
        />
        <PanelBody flush>
          <ResourceView resource={risk} loadingRows={6}>
            {(data) => {
              if (isUnavailable(data.exposure)) {
                return (
                  <div className="p-3">
                    <UnavailableBlock subject={data.exposure.subject} reason={data.exposure.reason} />
                  </div>
                );
              }
              if (data.exposure.buckets.length === 0) {
                return (
                  <div className="p-3">
                    <EmptyBlock headline="No exposure is recorded on any axis.">
                      <p>
                        Observed, not assumed: <code className="num">GET /api/v1/risk</code> served
                        the exposure section and it holds zero buckets. The book is flat, not
                        unreadable.
                      </p>
                    </EmptyBlock>
                  </div>
                );
              }
              return (
                <TableWell maxHeight="40vh" label="Exposure buckets">
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
                      </tr>
                    </thead>
                    <tbody>
                      {data.exposure.buckets.map((bucket) => (
                        <tr
                          key={`${bucket.axis}:${bucket.bucket}`}
                          data-alert={bucket.breached ? "true" : undefined}
                        >
                          <td className="num text-[color:var(--color-ink-dim)]">{bucket.axis}</td>
                          <td>{bucket.bucket}</td>
                          <td className="n">{formatDecimal(bucket.gross)}</td>
                          <td className="n">{formatDecimal(bucket.net)}</td>
                          <td className="n" data-direction={bucket.breached ? "negative" : undefined}>
                            {formatPercent(bucket.share)}
                          </td>
                          <td className="n text-[color:var(--color-ink-dim)]">
                            {formatPercent(bucket.limit)}
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

      <Panel>
        <PanelHead
          title="Concentration findings"
          meta={<Freshness resource={risk} name="concentrations" />}
        />
        <PanelBody flush>
          <ResourceView resource={risk} loadingRows={4}>
            {(data) => {
              if (isUnavailable(data.concentrations)) {
                return (
                  <div className="p-3">
                    <UnavailableBlock
                      subject={data.concentrations.subject}
                      reason={data.concentrations.reason}
                    />
                  </div>
                );
              }
              if (data.concentrations.findings.length === 0) {
                return (
                  <div className="p-3">
                    <EmptyBlock headline="No concentration finding is raised.">
                      <p>
                        The concentrations section was served and holds zero findings — a measured
                        clean bill, not a panel that could not be read.
                      </p>
                    </EmptyBlock>
                  </div>
                );
              }
              return (
                <TableWell maxHeight="34vh" label="Concentration findings">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Axis</th>
                        <th scope="col">Bucket</th>
                        <th scope="col" className="n">
                          Gross
                        </th>
                        <th scope="col" className="n">
                          Share
                        </th>
                        <th scope="col" className="n">
                          Limit
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.concentrations.findings.map((finding) => (
                        <tr key={`${finding.axis}:${finding.bucket}`} data-alert="true">
                          <td className="num text-[color:var(--color-ink-dim)]">{finding.axis}</td>
                          <td>{finding.bucket}</td>
                          <td className="n">{formatDecimal(finding.gross)}</td>
                          <td className="n" data-direction="negative">
                            {formatPercent(finding.share)}
                          </td>
                          <td className="n text-[color:var(--color-ink-dim)]">
                            {formatPercent(finding.limit)}
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

      <Panel>
        <PanelHead title="Limit utilisation" meta={<Freshness resource={risk} name="limit utilisation" />} />
        <PanelBody>
          <ResourceView resource={risk} loadingRows={2}>
            {(data) => (
              <div className="flex flex-col gap-2">
                <UnavailableBlock
                  subject={data.limit_utilisation.subject}
                  reason={data.limit_utilisation.reason}
                />
                <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                  A limit whose utilisation cannot be read is a limit nobody can see approaching.
                  The bounds below are real and enforced upstream, but until the platform serves a
                  utilisation figure for them, the first sign of one being near will be it firing.
                </p>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="The limits that do exist"
          meta={<Freshness resource={capital} name="capital bounds" />}
        />
        <PanelBody>
          <ResourceView resource={capital} loadingRows={4}>
            {(c) => (
              <div className="flex flex-col gap-3">
                <KpiRow>
                  <Kpi
                    label="Envelopes issued"
                    value={formatCount(c.envelopes.length)}
                    note="grants cut from the bounds below"
                  />
                  <Kpi
                    label="Outstanding recalls"
                    value={formatCount(c.outstanding_recalls.length)}
                    tone={c.outstanding_recalls.length > 0 ? "bad" : "ok"}
                    note="issued and not yet acknowledged"
                  />
                </KpiRow>
                <dl className="flex flex-col">
                  <KeyValue label="Total budget">{formatDecimal(c.bounds.total_budget)}</KeyValue>
                  <KeyValue label="Per strategy">{formatDecimal(c.bounds.per_strategy)}</KeyValue>
                  <KeyValue label="Per cell">{formatDecimal(c.bounds.per_cell)}</KeyValue>
                  <KeyValue label="Per venue">{formatDecimal(c.bounds.per_venue)}</KeyValue>
                </dl>
                <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                  These capital bounds are the limits enforced today, shown with the count of
                  envelopes issued against them. Utilisation ratios are not computed here: money
                  arrives as decimal strings and this console never parses them into numbers, so a
                  ratio the platform does not serve is a ratio this page does not show.
                </p>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
