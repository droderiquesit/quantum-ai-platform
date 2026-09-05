"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { formatCount, formatDecimal } from "@/lib/format";
import { useSelfModel, type SelfModelComponent } from "@/lib/hooks/useCognition";
import { CognitionHeader, Muted } from "../_shared";

/**
 * The self-model, read-only: what the platform has measured about the
 * accuracy of each of its own origins.
 *
 * `GET /cognition/self-model` answers one `CapabilityEstimate` per detector,
 * analyst, rung and strategy family, from a bounded window of graded
 * outcomes, and the sample below which it refuses to estimate. This page
 * renders those rows in the order they came and nothing derived from them:
 * it does not sort, rank, average or shrink, because every one of those is a
 * judgement the platform has already made and recorded, and a page that made
 * it again would be a second self-model that could disagree with the first.
 *
 * A row whose `accuracy` is `null` is the platform refusing an estimate it
 * has too few outcomes to hold. It is rendered as that refusal, naming the
 * threshold, rather than as a blank or a zero — a zero would read as an
 * origin measured to be always wrong, which is the opposite of unmeasured.
 */
export default function SelfModelPage() {
  const model = useSelfModel();

  return (
    <div className="flex flex-col gap-3 p-3">
      <CognitionHeader
        title="Self-model"
        reads="GET /cognition/self-model"
        meta={<Freshness resource={model} name="self-model" />}
      />

      <Panel>
        <PanelHead title="Capability estimates, per origin" />
        <PanelBody>
          <ResourceView resource={model} loadingRows={4}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Components"
                    value={<span data-testid="self-model-count">{formatCount(data.components.length)}</span>}
                    note="GET /cognition/self-model: components"
                  />
                  <Kpi
                    label="Calibrated"
                    value={formatCount(data.components.filter((component) => component.calibrated).length)}
                    note="rows the platform marked calibrated; its flag, not a count of non-null accuracies"
                    tone="info"
                  />
                  <Kpi
                    label="Minimum sample"
                    value={<span data-testid="self-model-minimum">{formatCount(data.minimum_sample)}</span>}
                    note="below this many graded outcomes the platform refuses an estimate"
                  />
                </KpiRow>
                {data.components.length === 0 ? (
                  <div className="mt-3" data-testid="self-model-empty">
                    <EmptyBlock headline="The self-model holds no component.">
                      <p>
                        No origin has been graded in this process. A component appears once the LEARN
                        stage has resolved a thesis that origin contributed to — this is an observed empty
                        model, not an unread one.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <div className="mt-3">
                    <TableWell maxHeight="560px" label="capability estimate per origin, in the platform's order">
                      <table className="dt" data-testid="self-model-table">
                        <thead>
                          <tr>
                            <th scope="col">Kind</th>
                            <th scope="col">Key</th>
                            <th scope="col" className="n">
                              Samples
                            </th>
                            <th scope="col" className="n">
                              Accuracy
                            </th>
                            <th scope="col">Calibrated</th>
                          </tr>
                        </thead>
                        <tbody>
                          {data.components.map((component, index) => (
                            <ComponentRow
                              key={`${component.kind}:${component.key}:${index}`}
                              component={component}
                              minimumSample={data.minimum_sample}
                            />
                          ))}
                        </tbody>
                      </table>
                    </TableWell>
                  </div>
                )}
                <p className="mt-2">
                  <Muted>
                    Rows are in the order the platform answered them. Accuracy is the platform&rsquo;s
                    own estimate, shrunk by its own pseudo-counts; this page does not average, rank or
                    re-estimate it, and a row without one is a refusal the platform made.
                  </Muted>
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

function ComponentRow({ component, minimumSample }: { component: SelfModelComponent; minimumSample: number }) {
  const refused = component.accuracy === null;
  return (
    <tr data-testid="self-model-row" data-refused={refused ? "true" : undefined}>
      <td>
        <Chip tone="info">{component.kind}</Chip>
      </td>
      <td className="num">{component.key}</td>
      <td className="n">{formatCount(component.samples)}</td>
      <td className="n" data-testid="self-model-accuracy">
        {refused ? (
          <Muted>below minimum sample (n &lt; {formatCount(minimumSample)})</Muted>
        ) : (
          formatDecimal(component.accuracy)
        )}
      </td>
      <td>
        <Chip tone={component.calibrated ? "ok" : "neutral"}>{component.calibrated ? "calibrated" : "uncalibrated"}</Chip>
      </td>
    </tr>
  );
}
