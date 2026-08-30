import type { Metadata } from "next";
import { AlgorikMark } from "@algorik/brand";
import { Card, CardGrid, Column, CtaBand, CtaLink, Section } from "../ui";

export const metadata: Metadata = {
  title: "Algorithmic intelligence with an audit trail",
  description:
    "Algorik is a paper-trading research platform: an eight-stage reasoning loop, deterministic risk checks, and a hash-chained log that makes every decision reproducible.",
};

/**
 * The eight stages, one honest sentence each. The names are the kernel's own
 * (`crates/runtime/qip-kernel/src/cycle.rs`), not marketing inventions.
 */
const STAGES = [
  {
    name: "Sense",
    text: "Market and reference data are absorbed with bounded retention, each fact stamped with the instant it became knowable.",
  },
  {
    name: "Understand",
    text: "Normalised events become regimes, correlations and context the rest of the loop can reason over.",
  },
  {
    name: "Discover",
    text: "Candidate opportunities are detected and scored; most are discarded, and the discard reason is recorded.",
  },
  {
    name: "Reason",
    text: "A panel of specialist agents examines each surviving opportunity; every finding carries its evidence and a computed confidence.",
  },
  {
    name: "Simulate",
    text: "Proposals are rehearsed against a simulator before anything is decided about them.",
  },
  {
    name: "Decide",
    text: "Deterministic risk and capital checks accept or veto — and a veto is a recorded outcome, never a silent drop.",
  },
  {
    name: "Act",
    text: "Accepted orders execute against simulated venues only. There is no live path.",
  },
  {
    name: "Learn",
    text: "Outcomes are attributed back to the reasoning that produced them, and the scoring feeds the next cycle.",
  },
] as const;

export default function WelcomePage() {
  return (
    <>
      <Hero />

      <Section
        eyebrow="What Algorik is"
        title="One platform, three disciplines"
        lede="Reasoning, governance and infrastructure are built together, because a conclusion nobody can bound, attribute or reproduce is not research."
      >
        <CardGrid>
          <Card title="Intelligence">
            <p>
              Specialist reasoning agents — macro, equities, credit, microstructure, derivatives
              and more — review every opportunity, with an adversarial reviewer whose job is to
              disagree.
            </p>
            <p>Confidence is computed arithmetic over evidence, never an adjective.</p>
          </Card>
          <Card title="Risk &amp; Governance">
            <p>
              Deterministic pre-trade checks run before an order object exists, and they never
              route through a model. Limits, capital envelopes and a scoped kill switch bound
              everything the platform may do.
            </p>
            <p>Every agent on the roster is subject to governance review, on a clock.</p>
          </Card>
          <Card title="Infrastructure">
            <p>
              Rust services with explicit timeouts, regional execution cells that keep working
              within their capital envelope when the centre is unreachable, and an event log that
              is hash-chained so history cannot be quietly edited.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        raised
        eyebrow="The loop"
        title="How Algorik reasons"
        lede="Eight stages run in one cycle, and each stage reports what it produced and why — a cycle that traded and a cycle where nothing cleared the bar are equally legible afterwards."
      >
        <ol className="grid gap-4 md:grid-cols-2">
          {STAGES.map((stage, index) => (
            <li
              key={stage.name}
              className="rounded-[8px] border border-[color:var(--color-line)] bg-[color:var(--color-void)] p-5"
            >
              <div className="flex items-baseline gap-3">
                <span className="num text-[12px] text-[color:var(--color-ink-faint)]">
                  {String(index + 1).padStart(2, "0")}
                </span>
                <h3 className="text-[13px] font-semibold uppercase tracking-[0.07em]">
                  {stage.name}
                </h3>
              </div>
              <p className="mt-2 text-[13px] leading-[1.6] text-[color:var(--color-ink-dim)]">
                {stage.text}
              </p>
            </li>
          ))}
        </ol>
      </Section>

      <Section
        eyebrow="Transparency"
        title="Every decision reproducible from the log"
        lede="The platform is built for the question that matters after the fact: not just what it decided, but why, on what evidence, at what cost — and whether the reasoning held up."
      >
        <CardGrid>
          <Card title="Hash-chained events">
            <p>
              Every record links to the one before it. Nothing can write an event that cannot be
              replayed, and nothing can edit history that has been sealed.
            </p>
          </Card>
          <Card title="Exact attribution">
            <p>
              Outcomes are attributed to the decisions that produced them, and costs are billed to
              what actually ran — not to what was planned.
            </p>
          </Card>
          <Card title="Legible refusals">
            <p>
              A vetoed proposal, a discarded opportunity and a limit that fired are recorded
              outcomes with named reasons, so a quiet day is as auditable as a busy one.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <CtaBand
        title="Research with a record"
        lede="Open an account, watch the loop run against simulated venues, and read the log that explains every step it took."
      />
    </>
  );
}

function Hero() {
  return (
    <section className="border-b border-[color:var(--color-line)] bg-[color:var(--color-surface)]">
      <Column className="pb-20 pt-16">
        {/* The gradient icon rather than the ink lockup: the mark reads on
            both themes, and the header already carries the full wordmark. */}
        <AlgorikMark size={56} />
        <p className="eyebrow mt-8">Paper-trading research platform</p>
        <h1 className="mt-3 max-w-[820px] text-[34px] font-semibold leading-[1.08] tracking-[-0.02em] sm:text-[44px]">
          Algorithmic intelligence with an audit trail.
        </h1>
        <p className="mt-5 max-w-[680px] text-[15px] leading-[1.65] text-[color:var(--color-ink-dim)]">
          Algorik senses markets, reasons about them with a panel of specialist agents, and
          executes only against a simulator. Paper trading is structural, not a setting — no live
          order can be submitted — and every decision is reproducible from a hash-chained log.
        </p>
        <div className="mt-8 flex flex-wrap items-center gap-3">
          <CtaLink href="/sign-up" variant="primary">
            Get started
          </CtaLink>
          <CtaLink href="/platform">See the platform</CtaLink>
        </div>
      </Column>
    </section>
  );
}
