"use client";

import { Chip, Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, UnavailableBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Models, Quantum } from "@/lib/api/types";
import { formatCount, formatMicros } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * What the platform routes to a quantum machine, and the rule that governs it.
 *
 * Everything on this page is the platform speaking for itself. The routing
 * facts and their note come from GET /quantum verbatim; the job history is an
 * absence the platform states, rendered as the absence it is; and the spend
 * figures are the observed-use ledger, billed as consumed. The one thing this
 * page adds is the standing rule the numbers sit under — and even there it
 * quotes the platform rather than asserting a result on its behalf, because a
 * research page that vouches for quantum benefit the platform has not measured
 * is exactly the claim ADR 0006 exists to prevent.
 */
export default function QuantumExperimentsPage() {
  const quantum = useResource<Quantum>(platform.quantum, {
    key: "research-quantum",
    label: "GET /quantum",
    intervalMs: 30_000,
  });
  const models = useResource<Models>(platform.models, {
    key: "research-quantum-models",
    label: "GET /models",
    intervalMs: 20_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Routing"
            meta={<Freshness resource={quantum} name="quantum" />}
            actions={<Chip>GET /api/v1/quantum</Chip>}
          />
          <PanelBody>
            <ResourceView resource={quantum} loadingRows={4}>
              {(data) => (
                <div className="flex flex-col gap-2">
                  <dl className="flex flex-col">
                    <KeyValue label="Provider">{data.routing.provider}</KeyValue>
                    <KeyValue label="Classical baseline">{data.routing.classical_baseline}</KeyValue>
                  </dl>
                  <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                    {data.routing.note}
                  </p>
                  {/* Jobs is a field-level absence inside an otherwise-answered
                      body, so ResourceView cannot render it; it is stated here
                      in the platform's own words. */}
                  <UnavailableBlock subject={data.jobs.subject} reason={data.jobs.reason} />
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="Compute spend, as observed"
            meta={<Freshness resource={models} name="models" />}
            actions={<Chip>GET /api/v1/models</Chip>}
          />
          <PanelBody>
            <ResourceView resource={models} loadingRows={4}>
              {(data) => (
                <div className="flex flex-col gap-2">
                  <KpiRow>
                    <Kpi
                      label="Agent runs"
                      value={formatCount(data.observed_use.agent_runs)}
                      note="an agent was asked a question"
                    />
                    <Kpi
                      label="Model calls"
                      value={formatCount(data.observed_use.model_calls)}
                      note="requests that reached a model"
                    />
                    <Kpi
                      label="Tokens"
                      value={formatCount(data.observed_use.tokens)}
                      note="billed as consumed, not as planned"
                    />
                    <Kpi
                      label="Cost"
                      value={formatMicros(data.observed_use.cost_micros)}
                      unit="units"
                      note="charged in micros and kept integral"
                    />
                  </KpiRow>
                  <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    The spend any quantum experiment competes against. These figures are the ledger
                    of what actually ran, and a quantum run that cannot beat the classical baseline
                    is spend without a finding.
                  </p>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead title="The standing rule" actions={<Chip tone="info">ADR 0006</Chip>} />
        <PanelBody>
          <ResourceView resource={quantum} loadingRows={3}>
            {(data) => (
              <div className="flex max-w-[86ch] flex-col gap-2">
                <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
                  A quantum method runs here only where a measured benefit against the classical
                  baseline is demonstrable (ADR 0006). The baseline is computed on every routed
                  problem, every time — so a quantum job without a classical result beside it cannot
                  exist, and a page reporting one would be reporting a defect. In the words the
                  platform itself serves for this route:
                </p>
                <blockquote className="border-l-2 border-[color:var(--color-line-strong)] pl-3 text-[12px] italic leading-relaxed text-[color:var(--color-ink)]">
                  {data.routing.note}
                </blockquote>
                <p className="num text-[10px] text-[color:var(--color-ink-faint)]">
                  — GET /api/v1/quantum, routing.note
                </p>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
