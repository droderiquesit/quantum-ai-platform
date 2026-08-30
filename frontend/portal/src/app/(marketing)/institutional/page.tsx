import type { Metadata } from "next";
import { Card, CardGrid, CtaBand, PageIntro, Section } from "../ui";

export const metadata: Metadata = {
  title: "Institutional",
  description:
    "Algorik for research and risk desks: reproducible decisions, exact attribution, governed agents and capital controls that fire before an order exists.",
};

export default function InstitutionalPage() {
  return (
    <>
      <PageIntro
        eyebrow="Institutional"
        title="Built for the desk that has to explain itself."
        lede="Algorik is shaped by one question a research or risk desk gets asked after the fact: why did the system do that? Every capability below exists so the answer is a record, not a reconstruction."
      />

      <Section
        eyebrow="Reproducibility"
        title="The decision, replayable"
        lede="A backtest you cannot reproduce is an anecdote. Algorik treats the production loop the same way."
      >
        <CardGrid columns={2}>
          <Card title="From the log alone">
            <p>
              Every decision is reproducible from the hash-chained event log: the inputs as they
              were knowable at the time, the reasoning that ran, the checks that fired, and the
              simulated execution that followed.
            </p>
          </Card>
          <Card title="Bitemporal by design">
            <p>
              Data carries both the instant a fact was true and the instant it became knowable, so
              &ldquo;what did we know then&rdquo; has an exact answer and point-in-time leakage is
              a detectable defect rather than a quiet flattering of results.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        raised
        eyebrow="Attribution"
        title="Who decided, on what, at what cost"
      >
        <CardGrid>
          <Card title="Exact attribution">
            <p>
              Outcomes attribute back to the specific findings and decisions that produced them —
              expected against realised, per strategy, per cycle. Costs are billed to what ran,
              not to what was planned.
            </p>
          </Card>
          <Card title="Governed agents">
            <p>
              The reasoning roster is subject to governance review: each agent&apos;s
              authorisation is checked on a clock, findings are recorded, and the review is
              readable from the API rather than living in a slide deck.
            </p>
          </Card>
          <Card title="Legible confidence">
            <p>
              Confidence is derived arithmetic with stated combination rules. A committee
              reviewing a decision sees the same number the platform used, computed the same way.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        eyebrow="Control"
        title="Capital that cannot wander"
        lede="Limits that cannot fire are not controls. Algorik's fire before an order object exists."
      >
        <CardGrid>
          <Card title="Envelopes, not balances">
            <p>
              Capital is granted to strategies and regional cells as time-bounded envelopes with
              explicit bounds and recall. A cell that loses contact with the centre keeps working
              inside its envelope — and only inside it.
            </p>
          </Card>
          <Card title="Deterministic pre-trade checks">
            <p>
              Every pre-trade check is deterministic and never routed to a model. The same book
              and the same proposal produce the same verdict, every time, which is what makes the
              verdict reviewable.
            </p>
          </Card>
          <Card title="Scoped kill switch">
            <p>
              Trading can be halted by scope, with an authenticated operator identity and a typed
              reason on every halt and every recovery. The audit trail is the mechanism, not an
              afterthought.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <CtaBand
        title="Talk to us about your desk"
        lede="Algorik is paper trading, structurally. If reproducibility, attribution and governance are what your evaluation turns on, we should talk."
        primaryHref="/company#contact"
        primaryLabel="Contact us"
        secondaryHref="/security"
        secondaryLabel="Security overview"
      />
    </>
  );
}
