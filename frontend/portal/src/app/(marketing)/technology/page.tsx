import type { Metadata } from "next";
import { Bullets, Card, CardGrid, CtaBand, PageIntro, Section } from "../ui";

export const metadata: Metadata = {
  title: "Technology",
  description:
    "How Algorik reasons: a seventeen-agent panel with computed confidence, and quantum experiments that are only kept when they measurably improve on a classical baseline.",
};

/** The reasoning roster, as the deep brain actually hosts it. */
const PANEL_AGENTS = [
  "Chief investment intelligence",
  "Macro analyst",
  "Equity analyst",
  "Credit analyst",
  "Microstructure analyst",
  "Derivatives analyst",
  "Commodities analyst",
  "FX and rates analyst",
  "Alternative data analyst",
  "Causal analyst",
  "Adversarial reviewer",
  "Simulation analyst",
  "Portfolio construction",
  "Quantum optimization",
  "Risk control",
  "Learning attribution",
  "Compliance control",
] as const;

export default function TechnologyPage() {
  return (
    <>
      <PageIntro
        eyebrow="Technology"
        title="Reasoning you can check, not reasoning you must trust."
        lede="Two research programmes share one discipline: every conclusion carries its evidence, its computed confidence and its cost, and every experimental method is measured against a boring baseline before it is believed."
      />

      <Section
        id="ai"
        eyebrow="AI intelligence"
        title="A seventeen-agent reasoning panel"
        lede="The deep brain hosts seventeen specialist agents and refuses to host the one agent that touches a venue — reasoning and execution are separated by construction, not by convention."
      >
        <div className="grid gap-8 lg:grid-cols-[1.2fr_1fr]">
          <div className="space-y-4 text-[14px] leading-[1.7] text-[color:var(--color-ink-dim)]">
            <p>
              Each agent examines an opportunity from its own discipline and returns findings with
              evidence attached. One agent failing does not silence the rest: dispatch is isolated
              per agent, because an organisation where one analyst&apos;s bug mutes the other
              seventeen is worse than one where a finding is missing.
            </p>
            <p>
              The adversarial reviewer exists to disagree. A panel that always concurs is not a
              panel; it is one opinion with sixteen echoes.
            </p>
            <p>
              <strong className="font-semibold text-[color:var(--color-ink)]">
                Confidence is arithmetic.
              </strong>{" "}
              A confidence figure is derived from the evidence by stated combination rules, so two
              screens can never disagree about how sure the platform is, and &ldquo;high
              conviction&rdquo; is never an adjective someone typed.
            </p>
            <p>
              Deterministic pre-trade risk checks never route to a model. The cost router makes
              that structural: where determinism is required, the routing type cannot even name a
              model rung.
            </p>
          </div>
          <div className="rounded-[14px] border border-[color:var(--color-line)] bg-[color:var(--color-surface)] p-6">
            <h3 className="eyebrow">The panel</h3>
            <ul className="mt-4 grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-2">
              {PANEL_AGENTS.map((agent) => (
                <li
                  key={agent}
                  className="text-[12.5px] leading-[1.5] text-[color:var(--color-ink-dim)]"
                >
                  {agent}
                </li>
              ))}
            </ul>
            <p className="mt-4 border-t border-[color:var(--color-line)] pt-3 text-[11.5px] leading-[1.6] text-[color:var(--color-ink-faint)]">
              The execution agent is deliberately absent: the node that reasons refuses, at
              start-up, to host anything holding a market-touching capability.
            </p>
          </div>
        </div>
      </Section>

      <Section
        id="quantum"
        raised
        eyebrow="Quantum research"
        title="A classical baseline, every time"
        lede="Algorik runs QAOA experiments for portfolio optimisation on a hosted quantum runtime, with a local steepest-descent fallback. The differentiator is the rule the experiments run under, recorded as ADR 0006."
      >
        <div className="grid gap-8 lg:grid-cols-[1.2fr_1fr]">
          <div className="space-y-4 text-[14px] leading-[1.7] text-[color:var(--color-ink-dim)]">
            <p>
              Every quantum job is paired with a classical baseline computed at the same time, on
              the same problem. Quantum methods stay in the loop only where a measured benefit
              against that baseline is demonstrable — and are removed where it is not.
            </p>
            <p className="text-[17px] font-semibold leading-[1.5] text-[color:var(--color-ink)]">
              &ldquo;We used a quantum computer&rdquo; is not a result.
            </p>
            <p>
              The comparison itself is readable from the API: quantum jobs and their classical
              baselines are served side by side, so the claim is a thing a person can check rather
              than a thing the platform asserts about itself.
            </p>
          </div>
          <div className="rounded-[14px] border border-[color:var(--color-line)] bg-[color:var(--color-surface)] p-6">
            <h3 className="eyebrow">What that rules out</h3>
            <div className="mt-4">
              <Bullets
                items={[
                  "Reporting a quantum run without its classical twin.",
                  "Keeping a quantum path because it is novel rather than because it measured better.",
                  "Describing hardware used as if it were performance achieved.",
                ]}
              />
            </div>
          </div>
        </div>
      </Section>

      <Section
        eyebrow="Engineering"
        title="Deliberately boring where it counts"
      >
        <CardGrid>
          <Card title="Rust, end to end">
            <p>
              Every latency-sensitive and core service is Rust, with blocking I/O and an explicit
              timeout on every call that leaves the process. The browser layer is the one
              deliberate exception.
            </p>
          </Card>
          <Card title="A tiny supply chain">
            <p>
              The core platform admits two third-party libraries, total. Every additional
              dependency is an architecture decision with a written record, because a dependency
              is an audit surface, not a download.
            </p>
          </Card>
          <Card title="Determinism where money moves">
            <p>
              Money is decimal arithmetic, iteration orders are stable, and a replay of the log is
              a replay — not an approximation that usually agrees.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <CtaBand
        title="Read the reasoning, not a summary of it"
        lede="Findings, confidence arithmetic and baseline comparisons are all part of the record the platform keeps."
        secondaryHref="/security"
        secondaryLabel="How it is secured"
      />
    </>
  );
}
