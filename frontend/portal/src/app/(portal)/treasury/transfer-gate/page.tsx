"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, StateBlock } from "@/components/data/States";
import { formatCount, formatTimestamp } from "@/lib/format";
import { useTransferGate, type GateAssessment, type GateCheck, type GateCheckName } from "@/lib/hooks/useTreasury";
import { Muted, Quote, TreasuryHeader } from "../_shared";

/**
 * The transfer gate (blueprint §37.3): seven deterministic checks, each of
 * which can only veto, with nothing behind it.
 *
 * `GET /transfer-gate` answers the seven checks in the order the gate runs
 * them, the last assessment it recorded if any, the kill switch the seventh
 * check reads, and `executes: false`. The checks are a static list — they
 * are what the gate is, not what it did — so this page holds the seven names
 * and §37.3's description of each, and cross-checks them against the roster
 * the route answered: a platform whose gate had grown an eighth check, or
 * lost one, would be reported rather than quietly drawn over.
 *
 * The last assessment is rendered as the platform recorded it: an approved
 * record, which carries no way to execute, or a vetoed record naming the
 * check that fired. Where the platform has assessed nothing, the page says
 * "no assessment yet" rather than showing seven green ticks that nothing
 * earned.
 *
 * There is no control here that composes an intent or submits one for
 * assessment. The gate is exercised by the platform against values the
 * platform supplies; this page is the record of that and cannot be its
 * input.
 */

/** `GateCheck::ALL`, in assessment order, with §37.3's description of each. */
const CHECKS: readonly { readonly name: GateCheckName; readonly description: string }[] = [
  {
    name: "corridor_authority",
    description:
      "Corridor active, signature record present and covering the current definition, destination allowlisted and usable, custody class permitted.",
  },
  {
    name: "caps",
    description: "Within the per-transfer, hourly, daily and cumulative caps, and inside permitted hours.",
  },
  {
    name: "minimum_interval",
    description: "The minimum interval has elapsed since the corridor last carried anything.",
  },
  {
    name: "stated_purpose",
    description: "The transfer reduces deviation from the optimiser's target — a purpose stated as arithmetic, not prose.",
  },
  {
    name: "source_balance",
    description: "The source balance is sufficient after reservations, in-flight settlement and commitments.",
  },
  {
    name: "velocity_and_anomaly",
    description: "The velocity breaker is not tripped and the anomaly detector is clear.",
  },
  {
    name: "kill_switch",
    description: "The kill switch is not tripped.",
  },
];

export default function TransferGatePage() {
  const gate = useTransferGate();
  const answered = gate.outcome !== null && gate.outcome.kind === "ok";

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Transfer gate"
        reads="GET /transfer-gate"
        posture={gate.data?.posture ?? null}
        meta={<Freshness resource={gate} name="transfer gate" />}
      />

      <Panel>
        <PanelHead title="The seven checks, in the order the gate runs them" />
        <PanelBody>
          {/* The list is what the gate is, so it is drawn whether or not the
              route answered; the assessment column says which. */}
          <CheckList roster={gate.data?.checks ?? null} assessment={gate.data?.last_assessment ?? null} answered={answered} />
          {!answered && gate.outcome !== null ? (
            <div className="mt-3">
              <ResourceView resource={gate} loadingRows={1}>
                {() => null}
              </ResourceView>
            </div>
          ) : null}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Last assessment" />
        <PanelBody>
          <ResourceView resource={gate} loadingRows={2}>
            {(data) =>
              data.last_assessment === null ? (
                <div data-testid="gate-no-assessment">
                  <StateBlock tone="neutral" label="no assessment yet" headline="The gate has assessed no intent." compact>
                    <p>
                      No transfer intent has reached the gate in this process. That is an absence,
                      not a pass: nothing has been approved and nothing has been vetoed.
                    </p>
                  </StateBlock>
                </div>
              ) : (
                <Assessment assessment={data.last_assessment} />
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Kill switch, as the seventh check reads it" />
        <PanelBody>
          <ResourceView resource={gate} loadingRows={1}>
            {(data) => (
              <div className="flex flex-col gap-1" data-testid="gate-kill-switch" data-alert={data.kill_switch.halted ? "true" : undefined}>
                <div className="flex flex-wrap items-center gap-2">
                  <Chip tone={data.kill_switch.halted ? "bad" : "ok"}>
                    {data.kill_switch.halted ? "HALTED" : "armed, not tripped"}
                  </Chip>
                  <Muted>
                    {formatCount(data.kill_switch.halted_scopes.length)} scope(s) halted individually
                    {data.kill_switch.halted_scopes.length > 0 ? `: ${data.kill_switch.halted_scopes.join(", ")}` : ""}
                  </Muted>
                </div>
                {data.kill_switch.halted ? (
                  <Quote tone="bad">
                    tripped by {data.kill_switch.tripped_by ?? "unknown"} at {formatTimestamp(data.kill_switch.tripped_at)}:{" "}
                    {data.kill_switch.reason ?? "no reason recorded"} — every transfer is vetoed
                  </Quote>
                ) : null}
                <Muted>The same fact /risk and /system serve; this page reads it and cannot change it.</Muted>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What an approval reaches" />
        <PanelBody>
          <ResourceView resource={gate} loadingRows={1}>
            {(data) => (
              <div className="flex flex-col gap-1" data-testid="gate-executes">
                {data.executes ? (
                  <div role="alert" data-alert="true">
                    <Chip tone="bad">EXECUTES: TRUE — CONTRADICTS ADR 0021</Chip>
                    <Quote tone="bad">
                      The route answered that the gate executes. The contract fixes this at false and
                      the platform has no engine that could; stop and investigate the process serving
                      this route.
                    </Quote>
                  </div>
                ) : (
                  <Chip tone="ok">executes: false</Chip>
                )}
                <Quote>{data.note}</Quote>
                <Muted>served {formatTimestamp(data.served_at)}</Muted>
              </div>
            )}
          </ResourceView>
          <p className="mt-2">
            <Muted>
              An approved record carries no way to execute. No function on the platform takes one
              and moves capital, and no control on this page composes an intent for it to assess.
            </Muted>
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}

function CheckList({
  roster,
  assessment,
  answered,
}: {
  roster: readonly GateCheck[] | null;
  assessment: GateAssessment | null;
  answered: boolean;
}) {
  const rosterNames = roster === null ? null : roster.map((check) => check.name);
  const staticNames = CHECKS.map((check) => check.name);
  const rosterMatches =
    rosterNames === null ||
    (rosterNames.length === staticNames.length && rosterNames.every((name, index) => name === staticNames[index]));

  return (
    <>
      {rosterMatches ? null : (
        <div className="mb-2" data-testid="gate-roster-mismatch">
          <Quote tone="warn">
            The route&rsquo;s roster differs from the seven this page names: it answered{" "}
            {rosterNames === null ? "nothing" : rosterNames.join(", ")}. The list below is this
            console&rsquo;s; the platform&rsquo;s is what the gate actually runs.
          </Quote>
        </div>
      )}
      <ol className="flex flex-col gap-1" data-testid="gate-checks" aria-label="the seven checks of the transfer gate">
        {CHECKS.map((entry, index) => {
          const state = stateOf(entry.name, assessment);
          const fromRoute = roster?.find((check) => check.name === entry.name) ?? null;
          return (
            <li
              key={entry.name}
              className="grid items-start gap-3 border-b border-[color:var(--color-line)] py-1.5 last:border-b-0"
              style={{ gridTemplateColumns: "24px 200px 1fr 120px" }}
              data-testid={`gate-check-${entry.name}`}
              data-alert={state === "vetoed" ? "true" : undefined}
            >
              <span className="num text-[11px] text-[color:var(--color-ink-faint)]">{index + 1}.</span>
              <span className="num text-[12px]">
                {entry.name}
                {fromRoute?.alerts ? (
                  <span className="block">
                    <Muted>veto alerts a person</Muted>
                  </span>
                ) : null}
              </span>
              <span className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">{entry.description}</span>
              <span>
                {!answered ? (
                  <Chip>unread</Chip>
                ) : state === "none" ? (
                  <Chip>not assessed</Chip>
                ) : state === "passed" ? (
                  <Chip tone="ok">passed</Chip>
                ) : state === "vetoed" ? (
                  <Chip tone="bad">VETOED</Chip>
                ) : (
                  <Chip tone="neutral">not reached</Chip>
                )}
              </span>
            </li>
          );
        })}
      </ol>
    </>
  );
}

/**
 * What the last assessment says about one check. The gate stops at the first
 * veto, so on a veto the checks before the one named passed and the ones after
 * were not reached; on an approval all seven passed.
 */
function stateOf(name: GateCheckName, assessment: GateAssessment | null): "none" | "passed" | "vetoed" | "not_reached" {
  if (assessment === null) return "none";
  if (assessment.outcome === "approved") return "passed";
  const order = CHECKS.findIndex((entry) => entry.name === name);
  const fired = CHECKS.findIndex((entry) => entry.name === assessment.check);
  if (fired < 0) return "none";
  if (order === fired) return "vetoed";
  return order < fired ? "passed" : "not_reached";
}

function Assessment({ assessment }: { assessment: GateAssessment }) {
  if (assessment.outcome === "vetoed") {
    return (
      <div className="flex flex-col gap-1" data-testid="gate-assessment" data-alert="true">
        <div className="flex flex-wrap items-center gap-2">
          <Chip tone="bad">VETOED</Chip>
          <span className="num text-[12px]">by {assessment.check ?? "an unnamed check"}</span>
          {assessment.alert ? <Chip tone="warn">alerted a person</Chip> : null}
          <Muted>assessed {formatTimestamp(assessment.assessed_at)}</Muted>
        </div>
        <Quote tone="bad">{assessment.reason ?? "no reason recorded"}</Quote>
        <Muted>Checks after the one that fired were not run; a veto is a veto.</Muted>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-1" data-testid="gate-assessment">
      <div className="flex flex-wrap items-center gap-2">
        <Chip tone={assessment.outcome === "approved" ? "ok" : "warn"}>{assessment.outcome}</Chip>
        <Muted>assessed {formatTimestamp(assessment.assessed_at)}</Muted>
      </div>
      {assessment.reason === null ? null : <Quote>{assessment.reason}</Quote>}
      <Muted>
        An approval is a record that every check admitted the intent at that instant. It carries no
        way to execute, and nothing consumed it.
      </Muted>
    </div>
  );
}
