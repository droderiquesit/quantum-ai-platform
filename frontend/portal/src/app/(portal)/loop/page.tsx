"use client";

import Link from "next/link";
import { useCallback, useState } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Heatmap, type HeatCell } from "@/components/viz/primitives";
import { describeOutcome, platform, type ApiOutcome } from "@/lib/api/client";
import type { CycleReport, SystemMetrics } from "@/lib/api/types";
import { formatClock, formatCount } from "@/lib/format";
import { recordCycleReport } from "@/lib/hooks/useCycleReports";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The eight stages, and what each one produced on each run.
 *
 * The platform reports a cycle only in the response to the request that ran it;
 * it keeps no readable history of past cycles. So this page holds the reports
 * it triggered itself and says so. Anything it shows is a cycle a person on
 * this console ran, in this tab, since it was opened — which is the only
 * cycle history that exists anywhere outside the event log.
 *
 * The grid distinguishes "produced nothing" from "did not run". Those are
 * opposite facts about a stage and a single zero would conflate them.
 */

/** The loop's stages, in the order the kernel traverses them. */
const STAGES = [
  "sense",
  "understand",
  "discover",
  "reason",
  "simulate",
  "decide",
  "act",
  "learn",
] as const;

interface Run {
  readonly report: CycleReport;
  readonly at: number;
}

export default function LoopPage() {
  const metrics = useResource<SystemMetrics>(platform.systemMetrics, {
    key: "loop-metrics",
    label: "GET /system/metrics",
    intervalMs: 10_000,
  });

  const [busy, setBusy] = useState(false);
  const [runs, setRuns] = useState<readonly Run[]>([]);
  const [failure, setFailure] = useState<ApiOutcome<CycleReport> | null>(null);

  const run = useCallback(async () => {
    setBusy(true);
    try {
      const response = await platform.runCycle();
      const outcome = response.outcome;
      if (outcome.kind === "ok") {
        setFailure(null);
        const entry: Run = { report: outcome.data, at: response.receivedAt };
        // Newest last, so the grid reads left to right in time. Bounded: a
        // console left running all day must not grow a column per cycle.
        setRuns((previous) => [...previous, entry].slice(-24));
        // Handed to the tab-wide register so the dataflow page, which runs
        // no cycle of its own, can show the stages of this one.
        recordCycleReport(outcome.data, response.receivedAt);
      } else {
        setFailure(outcome);
      }
      metrics.refresh();
    } finally {
      setBusy(false);
    }
  }, [metrics]);

  const latest: Run | null = runs[runs.length - 1] ?? null;
  const columns = runs.map((entry) => `#${entry.report.cycle}`);
  const cells: HeatCell[] = [];
  for (const entry of runs) {
    const column = `#${entry.report.cycle}`;
    const seen = new Map(entry.report.stages.map((stage) => [stage.stage, stage]));
    for (const name of STAGES) {
      const stage = seen.get(name);
      cells.push({
        row: name,
        column,
        // null, not zero: a stage that did not run was not measured at zero.
        value: stage === undefined || !stage.ran ? null : stage.produced,
      });
    }
  }

  const problems = latest?.report.stages.flatMap((stage) => stage.problems) ?? [];

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Loop"
          meta={<Freshness resource={metrics} name="metrics" />}
          actions={
            <>
              <Link href="/loop/dataflow" className="btn" data-testid="loop-dataflow-link">
                Dataflow
              </Link>
              <button
                type="button"
                className="btn"
                data-variant="primary"
                onClick={run}
                disabled={busy}
                data-testid="run-cycle"
              >
                {busy ? "Running…" : "Run one cycle"}
              </button>
            </>
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Cycles (platform)"
              value={formatCount(metrics.data?.cycles)}
              note="since this process started"
            />
            <Kpi
              label="Cycles run here"
              value={formatCount(runs.length)}
              note="from this tab, this session"
            />
            <Kpi
              label="Every stage traversed"
              value={latest === null ? "—" : latest.report.traversed_every_stage ? "yes" : "no"}
              tone={
                latest === null ? "neutral" : latest.report.traversed_every_stage ? "ok" : "warn"
              }
              note={latest === null ? "no run yet" : `cycle ${latest.report.cycle}`}
            />
            <Kpi
              label="Problems on last run"
              value={latest === null ? "—" : formatCount(problems.length)}
              tone={problems.length > 0 ? "warn" : "ok"}
              note={latest === null ? "no run yet" : "reported by a stage, not inferred"}
            />
            <Kpi
              label="Archived on last run"
              value={
                latest === null
                  ? "—"
                  : latest.report.archived === null
                    ? "none"
                    : formatCount(latest.report.archived)
              }
              tone={latest !== null && latest.report.archived === null ? "warn" : "neutral"}
              note="records sealed into the log"
            />
          </KpiRow>
          <p className="mt-2 text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
            <code className="num">POST /api/v1/cycle</code> advances the loop once and returns the
            stages it traversed. It requires the analyst role. The platform keeps no readable
            history of past cycles, so everything below is what this console itself ran.
          </p>
          {failure !== null ? (
            <p className="mt-2 text-[12px] text-[color:var(--color-down)]" role="alert">
              {describeOutcome(failure)}
            </p>
          ) : null}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Stages"
          meta={
            latest === null ? (
              <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                nothing run from this console
              </span>
            ) : (
              <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                cycle {latest.report.cycle} · {formatClock(latest.at)} ·{" "}
                {latest.report.correlation_id}
              </span>
            )
          }
        />
        <PanelBody>
          {latest === null ? (
            <EmptyBlock headline="No cycle has been run from this console.">
              <p>
                The eight stages below are the kernel&rsquo;s, in order. Run a cycle to see what
                each one produced and what it refused.
              </p>
              <ol className="mt-2 flex flex-wrap gap-1.5">
                {STAGES.map((stage) => (
                  <li key={stage}>
                    <Chip>{stage}</Chip>
                  </li>
                ))}
              </ol>
            </EmptyBlock>
          ) : (
            <ol className="grid grid-cols-[repeat(auto-fit,minmax(215px,1fr))] gap-2">
              {latest.report.stages.map((stage, index) => (
                <li
                  key={stage.stage}
                  className="flex flex-col gap-1 border border-[color:var(--color-line)] bg-[color:var(--color-surface)] px-3 py-2"
                  data-alert={stage.problems.length > 0 ? "true" : undefined}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="eyebrow">
                      {index + 1}. {stage.stage}
                    </span>
                    <Chip tone={!stage.ran ? "warn" : stage.problems.length > 0 ? "bad" : "ok"}>
                      {stage.ran ? `${stage.produced} produced` : "did not run"}
                    </Chip>
                  </div>
                  <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                    {stage.detail}
                  </p>
                  {stage.problems.length > 0 ? (
                    <p className="text-[11.5px] leading-relaxed text-[color:var(--color-down)]">
                      {stage.problems.join("; ")}
                    </p>
                  ) : null}
                </li>
              ))}
            </ol>
          )}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What each stage produced, per cycle" />
        <PanelBody>
          {runs.length === 0 ? (
            <EmptyBlock headline="Nothing to grid yet." />
          ) : (
            <Heatmap
              cells={cells}
              rows={[...STAGES]}
              columns={columns}
              label="stage output per cycle"
            />
          )}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Counters" meta={<Freshness resource={metrics} name="metrics" />} />
        <PanelBody flush>
          <ResourceView resource={metrics} loadingRows={4}>
            {(m) => (
              <TableWell maxHeight="none" label="Loop counters">
                <table className="dt">
                  <thead>
                    <tr>
                      <th scope="col">Counter</th>
                      <th scope="col" className="n">
                        Value
                      </th>
                      <th scope="col">Meaning</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr>
                      <td className="num">cycles</td>
                      <td className="n">{formatCount(m.cycles)}</td>
                      <td>loop iterations since the process started</td>
                    </tr>
                    <tr>
                      <td className="num">events_logged</td>
                      <td className="n">{formatCount(m.events_logged)}</td>
                      <td>records appended to the hash-chained log</td>
                    </tr>
                    <tr>
                      <td className="num">opportunities_queued</td>
                      <td className="n">{formatCount(m.opportunities_queued)}</td>
                      <td>DISCOVER&rsquo;s output awaiting REASON</td>
                    </tr>
                    <tr>
                      <td className="num">proposals</td>
                      <td className="n">{formatCount(m.proposals)}</td>
                      <td>theses that cleared the action bar</td>
                    </tr>
                    <tr>
                      <td className="num">orders</td>
                      <td className="n">{formatCount(m.orders)}</td>
                      <td>released by ACT to the simulator</td>
                    </tr>
                    <tr>
                      <td className="num">fills</td>
                      <td className="n">{formatCount(m.fills)}</td>
                      <td>returned by the simulated venue</td>
                    </tr>
                    <tr>
                      <td className="num">refusals</td>
                      <td className="n">{formatCount(m.refusals)}</td>
                      <td>risk said no before an order existed</td>
                    </tr>
                    <tr data-alert={m.live_fills ? "true" : undefined}>
                      <td className="num">live_fills</td>
                      <td className="n">{m.live_fills ? "true" : "false"}</td>
                      <td>whether any fill was not simulated</td>
                    </tr>
                  </tbody>
                </table>
              </TableWell>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
