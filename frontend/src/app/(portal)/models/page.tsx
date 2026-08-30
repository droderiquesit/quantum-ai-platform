"use client";

import { Chip, Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, StateBlock, UnavailableBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Models, Quantum } from "@/lib/api/types";
import { formatCount, formatMicros } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { describeWindow, useSeries } from "@/lib/hooks/useSeries";
import { AreaChart } from "@/components/viz/primitives";

/**
 * What the platform spent on models, and what it did on a quantum machine.
 *
 * Both endpoints answer partly with a stated absence, and this page renders the
 * absence rather than hiding the panel. That is the point of the screen: a desk
 * asking "which model decided this?" needs to see that the platform cannot
 * currently answer, in the place it would have answered.
 *
 * The quantum panel says the same thing from the other direction. The platform
 * computes a classical baseline on every routed problem (ADR 0006) and will not
 * report a quantum result without one — so "no jobs" here is a governed
 * absence, not a missing integration.
 */
export default function ModelsPage() {
  const models = useResource<Models>(platform.models, {
    key: "models",
    label: "GET /models",
    intervalMs: 20_000,
  });
  const quantum = useResource<Quantum>(platform.quantum, {
    key: "quantum",
    label: "GET /quantum",
    intervalMs: 30_000,
  });
  const training = useResource<unknown>(platform.training, {
    key: "training",
    label: "GET /training",
    intervalMs: 30_000,
  });

  const use = models.data?.observed_use ?? null;
  const calls = useSeries(use?.model_calls ?? null);
  const tokens = useSeries(use?.tokens ?? null);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Model use, as observed"
          meta={<Freshness resource={models} name="models" />}
          actions={<Chip>a record of use, not a roster</Chip>}
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Agent runs"
              value={formatCount(use?.agent_runs)}
              note="an agent was asked a question"
            />
            <Kpi
              label="Model calls"
              value={formatCount(use?.model_calls)}
              series={calls}
              trend="accent"
              note="requests that reached a model"
            />
            <Kpi
              label="Tokens"
              value={formatCount(use?.tokens)}
              series={tokens}
              trend="accent"
              note="billed as consumed, not as planned"
            />
            <Kpi
              label="Cost"
              value={formatMicros(use?.cost_micros)}
              unit="units"
              note="charged in micros and kept integral"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Model calls over time" meta={<Freshness resource={models} name="models" />} />
          <PanelBody>
            <AreaChart
              values={calls.values}
              label="model calls"
              height={150}
              caption={<>{describeWindow(calls)}. Accumulated by this tab from a counter.</>}
            />
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Registry" meta={<Freshness resource={models} name="registry" />} />
          <PanelBody>
            <ResourceView resource={models} loadingRows={4}>
              {(data) => (
                <div className="flex flex-col gap-2">
                  <UnavailableBlock
                    subject={data.registry.subject}
                    reason={data.registry.reason}
                  />
                  <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    A model registry with fit versions, reference feature samples and drift scores
                    exists in the Deep Brain, which runs as a separate process and serves no HTTP
                    surface. Until it does, the console cannot say which model produced a decision,
                    and it says that rather than showing an empty table.
                  </p>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Quantum" meta={<Freshness resource={quantum} name="quantum" />} />
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
                  <UnavailableBlock subject={data.jobs.subject} reason={data.jobs.reason} />
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Training" meta={<Freshness resource={training} name="training" />} />
          <PanelBody>
            <ResourceView resource={training} loadingRows={4}>
              {() => (
                <StateBlock
                  tone="info"
                  label="unmodelled"
                  headline="The platform returned a training body this console does not model."
                >
                  <p>
                    <code className="num">GET /api/v1/training</code> answered with data rather than
                    an absence. Nothing is rendered from it here, because a shape this page has not
                    been written against would have to be guessed at.
                  </p>
                </StateBlock>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
