"use client";

import { Chip, Freshness, Metric, MetricRow, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { isUnavailable, type SystemStatus } from "@/lib/api/types";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import {
  formatStatistic,
  useExploration,
  type Exploration,
  type LearnedKind,
  type OpenProbe,
} from "@/lib/hooks/useExploration";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Exploration budget: what the platform is spending to learn, and what it has
 * learned — §40.1's eighth surface, read from `GET /exploration`.
 *
 * Four things the route answers are rendered as it answered them, and the
 * page is organised around keeping them apart:
 *
 * * **The ceiling, or its declared absence.** The exploration share is a term
 *   of the desk's `Mandate`. Where the ledger holds no desk mandate the route
 *   says so with a reason and no default, and so does this page — a share of
 *   zero shown in that case would read as a mandate that explores nothing,
 *   which is a different fact from a mandate that does not exist.
 * * **Held, committed and spent, as three numbers.** Held is what the
 *   reservation ledger withholds from return-seeking capital; committed is
 *   what open probes still have against them; spend is what exploration has
 *   actually cost. Summing any two double-counts an open probe. Nothing on
 *   this page adds them.
 * * **The account's own counters**, so a reader can tell "nothing was ever
 *   bought" from "everything bought has settled".
 * * **Every open probe as the question it bought**, with the bound it may
 *   cost, the uncertainty it was opened against and when it expires.
 * * **What it learned, per kind, as the route's own table.** One row per
 *   `ProbeKind`, in the order the route sends them, including kinds the
 *   book has never bought — the route walks the enum rather than its
 *   records so that a kind never asked about is a row of zeroes and not a
 *   missing row that reads as nothing to report. A `measured_gain` of
 *   `null` is rendered as the words "not measured", never as `0`: the mean
 *   gain over zero probed settlements is not a number, and a figure there
 *   would tell a reader a question was answered with nothing learned.
 *
 * Should the route answer the table as an absence — the `Section`
 * convention every surface uses — its reason is rendered and nothing in its
 * place; and a body that is neither an absence nor the table this page knows
 * is shown verbatim under an "unread" label rather than through columns the
 * page would have to guess.
 *
 * Nothing on this page acts. "Adjust the exploration share" is the row's
 * "Acts on" cell in the blueprint and it is deliberately absent: the share is
 * a mandate term that changes through the capital path under an authenticated
 * operator, and the gateway refuses any write this console does not declare.
 */
export default function ExplorationPage() {
  const budget = useExploration();
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "exploration-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="exploration-page">
      <Panel>
        <PanelHead
          title="Exploration budget"
          meta={<Freshness resource={budget} name="exploration" />}
          actions={
            status.data === null ? null : (
              <StatusChip
                tone={status.data.live_capable ? "bad" : "ok"}
                label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
                title="GET /system/status: live_capable"
              />
            )
          }
        />
        <PanelBody>
          <p
            className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
            data-testid="exploration-declaration"
          >
            <span className="chip mr-2" data-tone="ok" data-testid="exploration-paper-label">
              PAPER TRADING
            </span>
            This page reads <span className="num">GET /exploration</span> and renders what it answered:
            the ceiling the desk mandate declares for learning, what learning is holding, committing
            and has cost, and every question the platform has an open probe against. Nothing here
            adjusts the share, opens a probe, settles one or submits an order: the share is a term of
            the desk&rsquo;s mandate and changes through the capital path under an authenticated
            operator, and every probe is bought and settled by the DECIDE and LEARN stages on the
            event log.
          </p>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="The ceiling" meta={<Freshness resource={budget} name="share" />} />
        <PanelBody>
          <ResourceView resource={budget} loadingRows={2}>
            {(data) => <ShareSection data={data} />}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What learning costs" meta={<Freshness resource={budget} name="budget" />} />
        <PanelBody>
          <ResourceView resource={budget} loadingRows={3}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Held"
                    value={<span data-testid="exploration-held">{formatDecimal(data.held)}</span>}
                    note="withheld from return-seeking capital by the reservation ledger"
                    tone="info"
                  />
                  <Kpi
                    label="Committed"
                    value={<span data-testid="exploration-committed">{formatDecimal(data.committed)}</span>}
                    note="what open probes still have against them"
                    tone="info"
                  />
                  <Kpi
                    label="Spent"
                    value={<span data-testid="exploration-spend">{formatDecimal(data.spend)}</span>}
                    note="what exploration has actually cost"
                    tone="info"
                  />
                </KpiRow>
                <p className="mt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                  Three answers to three questions, kept apart: adding held to committed, or
                  committed to spent, counts every open probe twice. The route sends all three as
                  the ledger holds them and this page totals nothing.
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="The account" meta={<Freshness resource={budget} name="account" />} />
        <PanelBody>
          <ResourceView resource={budget} loadingRows={1}>
            {(data) => (
              <MetricRow>
                <Metric
                  label="Open now"
                  value={<span data-testid="exploration-open-count">{formatCount(data.open_count)}</span>}
                  hint="probes still buying an answer"
                />
                <Metric
                  label="Opened, ever"
                  value={<span data-testid="exploration-opened-total">{formatCount(data.opened_total)}</span>}
                  hint="this is what tells never-bought from all-settled"
                />
                <Metric
                  label="Settled"
                  value={<span data-testid="exploration-settled-total">{formatCount(data.settled_total)}</span>}
                  hint="answered, on probed or observed evidence"
                />
                <Metric
                  label="Abandoned"
                  value={<span data-testid="exploration-abandoned-total">{formatCount(data.abandoned_total)}</span>}
                  hint="expired or withdrawn before an answer"
                />
                <Metric
                  label="Subjects forgotten"
                  value={<span data-testid="exploration-forgotten">{formatCount(data.subjects_forgotten)}</span>}
                  hint="aged out of the book's bounded retention"
                />
              </MetricRow>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Open probes" meta={<Freshness resource={budget} name="open probes" />} />
        <PanelBody>
          <ResourceView resource={budget} loadingRows={4}>
            {(data) =>
              data.open.length === 0 ? (
                <div data-testid="exploration-open-empty">
                  <EmptyBlock headline="No probe is open.">
                    <p>
                      Observed, not unread: the route answered and the open list is empty. The book
                      has opened <span className="num">{formatCount(data.opened_total)}</span>{" "}
                      probe(s) in all
                      {data.opened_total === 0
                        ? ", so the platform has never bought a question here."
                        : ", so every question bought has settled or was abandoned."}
                    </p>
                  </EmptyBlock>
                </div>
              ) : (
                <div className="flex flex-col gap-2" data-testid="exploration-open-list">
                  {data.open.map((probe) => (
                    <ProbeCard key={probe.id} probe={probe} />
                  ))}
                </div>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What it learned" meta={<Freshness resource={budget} name="learned" />} />
        <PanelBody>
          <ResourceView resource={budget} loadingRows={2}>
            {(data) => <LearnedSection data={data} />}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * The share the desk mandate declares, or the platform's statement that none
 * is declared. The second arm renders the route's own reason and no figure:
 * the handler refuses a default for the same reason the kernel's budget
 * refuses to run without one, and a `0` here would be this page supplying the
 * default the route would not.
 */
function ShareSection({ data }: { data: Exploration }) {
  if (isUnavailable(data.share)) {
    return (
      <div data-testid="exploration-share-absent">
        <StateBlock
          tone="warn"
          label="no ceiling declared"
          headline="The desk mandate declares no exploration share, so no ceiling is shown."
        >
          <p className="num" data-testid="exploration-share-reason">
            {data.share.reason}
          </p>
          <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
            The platform&rsquo;s own answer, under the subject{" "}
            <span className="num">{data.share.subject}</span>. No default is rendered in its place: a
            share shown here that the capital path would refuse would be a ceiling nobody declared.
          </p>
        </StateBlock>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <KpiRow>
        <Kpi
          label="Exploration share"
          value={<span data-testid="exploration-share">{data.share.share}</span>}
          note="the desk mandate's term, as a fraction of its capital, exactly as the mandate states it"
          tone="ok"
        />
      </KpiRow>
      <p className="max-w-[90ch] text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
        <Chip tone="neutral">
          <span data-testid="exploration-share-declared">declared</span>
        </Chip>{" "}
        Adjusting it is a change to the mandate, made through the capital path by an authenticated
        operator. This console offers no control for it and declares no write that could; the term is
        shown because the platform serves it, not because this page can move it.
      </p>
    </div>
  );
}

/**
 * The per-kind table, in the three states the wire can be in.
 *
 * The first is the route's table: one row per kind, transcribed column for
 * column from the handler's `learned.kinds`, totalled nowhere. The second is
 * the `Section` absence every surface may answer under — `available: false`
 * with a reason — rendered as that reason and nothing in its place. The
 * third is a body that is neither, which this page cannot have been written
 * against; it is rendered verbatim under an "unread" label rather than
 * through a shape guessed from a kernel struct, because a table drawn from
 * guessed columns would be this page inventing what the platform learned.
 */
function LearnedSection({ data }: { data: Exploration }) {
  if (isUnavailable(data.learned)) {
    return (
      <div data-testid="exploration-learned-absent">
        <StateBlock
          tone="warn"
          label="not served"
          headline="The platform serves no per-kind learning table in this deployment."
        >
          <p className="num" data-testid="exploration-learned-reason">
            {data.learned.reason}
          </p>
          <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
            Nothing is shown in its place. The mean information gain per probe kind is kept by the
            book; until the route serves it, a table here would be this console&rsquo;s guess.
          </p>
        </StateBlock>
      </div>
    );
  }
  if (!Array.isArray(data.learned.kinds)) {
    return (
      <div data-testid="exploration-learned-unread">
        <StateBlock
          tone="info"
          label="served, not yet read"
          headline="The platform served a body this console has not been taught the shape of."
        >
          <p>
            The body is shown as it arrived, so what the platform learned is on the screen rather
            than hidden behind a shape this page would otherwise have guessed.
          </p>
          <pre className="num mt-1.5 max-h-[40vh] overflow-auto text-[11px]" data-testid="exploration-learned-raw">
            {JSON.stringify(data.learned, null, 2)}
          </pre>
        </StateBlock>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2" data-testid="exploration-learned-table">
      <div className="overflow-x-auto">
        <table className="w-full text-[11.5px]">
          <thead>
            <tr className="eyebrow text-left">
              <th className="pr-3 pb-1">Kind</th>
              <th className="pr-3 pb-1 text-right">Opened</th>
              <th className="pr-3 pb-1 text-right">Probed</th>
              <th className="pr-3 pb-1 text-right">Observed</th>
              <th className="pr-3 pb-1 text-right">Gain, summed</th>
              <th className="pr-3 pb-1 text-right">Cost</th>
              <th className="pr-3 pb-1 text-right">Bound breaches</th>
              <th className="pb-1 text-right">Gain, per probe</th>
            </tr>
          </thead>
          <tbody>
            {data.learned.kinds.map((row) => (
              <LearnedRow key={row.kind} row={row} />
            ))}
          </tbody>
        </table>
      </div>
      <p className="max-w-[90ch] text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
        Every kind the platform can probe is a row, including the ones it has never bought: the
        route lists the enum, not the book&rsquo;s records, so a kind never asked about reads as
        zeroes rather than as a missing row. &ldquo;Gain, per probe&rdquo; is the platform&rsquo;s
        own mean over probed settlements and arrives as sent; where nothing has been probed it is
        not measured, and no figure is shown in its place. Nothing on this page sums a column.
      </p>
    </div>
  );
}

/**
 * One kind, column for column. `measured_gain` is the one cell that may be
 * `null`, and `null` is rendered as words rather than through
 * `formatStatistic`'s dash — a dash beside numeric neighbours reads as a
 * value the page failed to fetch, and this is a value the platform declined
 * to compute.
 */
function LearnedRow({ row }: { row: LearnedKind }) {
  return (
    <tr
      className="border-t border-[color:var(--color-line)] align-top"
      data-testid="exploration-learned-row"
      data-kind={row.kind}
    >
      <td className="py-1.5 pr-3">
        <div className="text-[12px] text-[color:var(--color-ink)]" data-testid="exploration-learned-learns">
          {row.learns}
        </div>
        <div className="num text-[10.5px] text-[color:var(--color-ink-faint)]" data-testid="exploration-learned-kind">
          {row.kind}
        </div>
      </td>
      <td className="num py-1.5 pr-3 text-right" data-testid="exploration-learned-opened">
        {formatCount(row.opened)}
      </td>
      <td className="num py-1.5 pr-3 text-right" data-testid="exploration-learned-probed">
        {formatCount(row.probed)}
      </td>
      <td className="num py-1.5 pr-3 text-right" data-testid="exploration-learned-observed">
        {formatCount(row.observed)}
      </td>
      <td className="num py-1.5 pr-3 text-right" data-testid="exploration-learned-probed-gain">
        {formatStatistic(row.probed_gain)}
      </td>
      <td className="num py-1.5 pr-3 text-right" data-testid="exploration-learned-realised-cost">
        {formatDecimal(row.realised_cost)}
      </td>
      <td className="num py-1.5 pr-3 text-right" data-testid="exploration-learned-bound-breaches">
        {formatCount(row.bound_breaches)}
      </td>
      <td className="py-1.5 text-right" data-testid="exploration-learned-measured-gain">
        {row.measured_gain === null ? (
          <span className="text-[color:var(--color-ink-faint)]">not measured</span>
        ) : (
          <span className="num">{formatStatistic(row.measured_gain)}</span>
        )}
      </td>
    </tr>
  );
}

/** One open probe: the question first, then the terms it was bought on. */
function ProbeCard({ probe }: { probe: OpenProbe }) {
  return (
    <article
      className="border border-[color:var(--color-line)] bg-[color:var(--color-surface)] p-3"
      data-testid="exploration-probe"
      data-probe={probe.id}
      data-kind={probe.kind}
      data-expires={probe.expires_at}
    >
      <header className="flex flex-wrap items-center gap-2">
        <span className="text-[13px] font-medium text-[color:var(--color-ink)]" data-testid="exploration-probe-learns">
          {probe.learns}
        </span>
        <Chip tone="info">
          <span data-testid="exploration-probe-kind">{probe.kind}</span>
        </Chip>
        <Chip tone="neutral">open</Chip>
      </header>
      <dl className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-[11.5px] sm:grid-cols-5">
        <Fact label="Subject" value={<span data-testid="exploration-probe-subject">{probe.subject}</span>} />
        <Fact
          label="May cost at most"
          value={<span data-testid="exploration-probe-bound">{formatDecimal(probe.maximum_loss)}</span>}
          note="the probe's own bound"
        />
        <Fact
          label="Uncertainty at open"
          value={<span data-testid="exploration-probe-uncertainty">{formatStatistic(probe.uncertainty_at_open)}</span>}
          note="the baseline its gain is measured against"
        />
        <Fact label="Opened" value={<span data-testid="exploration-probe-opened">{formatTimestamp(probe.opened_at)}</span>} />
        <Fact label="Expires" value={<span data-testid="exploration-probe-expires">{formatTimestamp(probe.expires_at)}</span>} />
      </dl>
      <p className="mt-2 text-[10.5px] leading-snug text-[color:var(--color-ink-faint)]">
        Probe <span className="num">{probe.id}</span>. Its settlement, and what it turned out to be
        worth, are the LEARN stage&rsquo;s to record.
      </p>
    </article>
  );
}

function Fact({ label, value, note }: { label: string; value: React.ReactNode; note?: string }) {
  return (
    <div className="min-w-0">
      <dt className="eyebrow truncate">{label}</dt>
      <dd className="num text-[13px] text-[color:var(--color-ink)]">{value}</dd>
      {note ? <p className="text-[10.5px] leading-snug text-[color:var(--color-ink-faint)]">{note}</p> : null}
    </div>
  );
}
