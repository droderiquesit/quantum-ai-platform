"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Portfolio } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The P&L page, framed around the fact that the platform currently declines
 * to serve one.
 *
 * Attribution is computed in-cycle and the book behind it is capability-gated,
 * so GET /pnl answers with a stated absence — and this page renders that
 * answer, in the platform's own words, where the curve would go. Money is
 * never illustrated on this console: a simulated P&L would be a number about
 * the desk itself that nobody measured, and it would be screenshotted within
 * the week. The real counts above the absence exist so the reader can see the
 * book is moving even while its value is not served.
 */
export default function PnlPage() {
  const portfolio = useResource<Portfolio>(platform.portfolio, {
    key: "pnl-portfolio",
    label: "GET /portfolio",
    intervalMs: 15_000,
  });
  const pnl = useResource<unknown>(platform.pnl, {
    key: "pnl",
    label: "GET /pnl",
    intervalMs: 30_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="The book the question is about"
          meta={<Freshness resource={portfolio} name="portfolio" />}
          actions={<Chip>GET /api/v1/portfolio</Chip>}
        />
        <PanelBody>
          <ResourceView resource={portfolio} loadingRows={2}>
            {(data) => (
              <KpiRow>
                <Kpi label="Proposals" value={formatCount(data.proposals)} note="staged by the loop" />
                <Kpi label="Orders" value={formatCount(data.orders)} note="all states" />
                <Kpi label="Fills" value={formatCount(data.fills)} note="recorded by this process" />
                <Kpi
                  label="Execution posture"
                  value={data.paper_only ? "PAPER TRADING" : "LIVE FILLS PRESENT"}
                  tone={data.paper_only ? "ok" : "bad"}
                  note="whether any fill came from a real venue"
                />
              </KpiRow>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Profit, loss and attribution"
          meta={<Freshness resource={pnl} name="P&L" />}
          actions={<Chip>GET /api/v1/pnl</Chip>}
        />
        <PanelBody>
          <ResourceView resource={pnl} loadingRows={3}>
            {() => (
              <StateBlock
                tone="info"
                label="unmodelled"
                headline="The platform returned a P&L body this console does not model."
              >
                <p>
                  <code className="num">GET /api/v1/pnl</code> answered with data rather than the
                  stated absence this page was written against. Nothing is rendered from it: a P&L
                  shape this console has not been written for would have to be guessed at, and a
                  guessed rendering of money is exactly what this page refuses to be.
                </p>
              </StateBlock>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What will render here" actions={<Chip tone="info">standing rule</Chip>} />
        <PanelBody>
          <div className="flex max-w-[86ch] flex-col gap-2 text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
            <p>
              When the platform serves this surface, this page renders realised profit and loss
              against the expected alpha each decision carried, and attribution per strategy — who
              or what earned each figure, traceable to the cycle that decided it. Attribution is
              computed in-cycle from the event log, so every number that will appear here is
              reproducible from the record rather than asserted by the page.
            </p>
            <p>
              Until then, the absence above is the whole answer, and it is a better one than a
              curve. An invented P&L would be a statement about money the desk owns that nobody
              measured — read in a meeting, screenshotted, and acted on before anyone asks where it
              came from. This console renders what the platform serves, states what it does not, and
              never illustrates money. That rule has no exception, and this page is where it would
              be tested.
            </p>
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}
