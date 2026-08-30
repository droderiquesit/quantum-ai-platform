"use client";

import { useCallback, useState } from "react";
import { describeOutcome, platform, type ApiOutcome } from "@/lib/api/client";
import type { CycleReport } from "@/lib/api/types";
import { formatClock } from "@/lib/format";
import { Panel, PanelBody, PanelHead } from "./Panel";
import { Chip } from "./Bits";

/**
 * Run one cycle of the intelligence loop, and report exactly what happened.
 *
 * `POST /api/v1/cycle` answers 202 with a stage-by-stage account, including the
 * stages that did not run and the problems each reported. All of it is shown:
 * a control that says only "done" teaches an operator to stop reading it.
 */
export function RunCycleCard({ onRan }: { onRan?: () => void }) {
  const [busy, setBusy] = useState(false);
  const [ranAt, setRanAt] = useState<number | null>(null);
  const [outcome, setOutcome] = useState<ApiOutcome<CycleReport> | null>(null);

  const run = useCallback(async () => {
    setBusy(true);
    try {
      const response = await platform.runCycle();
      setOutcome(response.outcome);
      setRanAt(response.receivedAt);
      onRan?.();
    } finally {
      setBusy(false);
    }
  }, [onRan]);

  return (
    <Panel>
      <PanelHead
        title="Intelligence loop"
        meta={
          ranAt === null ? (
            <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
              not run from this console
            </span>
          ) : (
            <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
              last run {formatClock(ranAt)}
            </span>
          )
        }
        actions={
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
        }
      />
      <PanelBody>
        <p className="mb-2 text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          <code className="num">POST /api/v1/cycle</code> advances the loop once and returns the
          stages it traversed. It requires the analyst role.
        </p>

        {outcome === null ? (
          <p className="text-[12px] text-[color:var(--color-ink-faint)]">
            No cycle has been run from this console in this session.
          </p>
        ) : outcome.kind !== "ok" ? (
          <p className="text-[12px] text-[color:var(--color-down)]" role="alert">
            {describeOutcome(outcome)}
          </p>
        ) : (
          <div className="flex flex-col gap-2">
            <div className="flex flex-wrap items-center gap-2">
              <Chip tone="info">cycle {outcome.data.cycle}</Chip>
              <Chip tone={outcome.data.halted ? "bad" : "ok"}>
                {outcome.data.halted ? "halted" : "ran"}
              </Chip>
              <Chip tone={outcome.data.traversed_every_stage ? "ok" : "warn"}>
                {outcome.data.traversed_every_stage ? "every stage" : "stages skipped"}
              </Chip>
              <Chip tone={outcome.data.archived === null ? "warn" : "ok"}>
                {outcome.data.archived === null
                  ? "not archived"
                  : `${outcome.data.archived} archived`}
              </Chip>
              <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                {outcome.data.correlation_id}
              </span>
            </div>
            {outcome.data.archive_error ? (
              <p className="text-[11.5px] text-[color:var(--color-down)]" role="alert">
                Archive error: {outcome.data.archive_error}
              </p>
            ) : null}
            <table className="dt">
              <thead>
                <tr>
                  <th scope="col">Stage</th>
                  <th scope="col">Ran</th>
                  <th scope="col" className="n">
                    Produced
                  </th>
                  <th scope="col">Detail</th>
                </tr>
              </thead>
              <tbody>
                {outcome.data.stages.map((stage) => (
                  <tr key={stage.stage} data-alert={stage.problems.length > 0 ? "true" : undefined}>
                    <td className="num">{stage.stage}</td>
                    <td>
                      <Chip tone={stage.ran ? "ok" : "neutral"}>{stage.ran ? "yes" : "no"}</Chip>
                    </td>
                    <td className="n">{stage.produced}</td>
                    <td className="whitespace-normal text-[11.5px] text-[color:var(--color-ink-dim)]">
                      {stage.detail}
                      {stage.problems.length > 0 ? (
                        <span className="block text-[color:var(--color-down)]">
                          {stage.problems.join("; ")}
                        </span>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </PanelBody>
    </Panel>
  );
}
