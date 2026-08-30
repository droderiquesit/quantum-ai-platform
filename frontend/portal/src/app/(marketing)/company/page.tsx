import type { Metadata } from "next";
import { Card, CardGrid, PageIntro, Section } from "../ui";

export const metadata: Metadata = {
  title: "Company",
  description:
    "What Algorik is — a paper-trading research platform — the operating principles it is built under, and how to reach the team.",
};

/**
 * The principles are the repository's real working rules, not marketing
 * inventions; each one names the failure it prevents.
 */
const PRINCIPLES = [
  {
    name: "Say why",
    text: "Code, commits and decisions name the failure they prevent. A record that restates what happened without why is worse than none, because it reads as an answer.",
  },
  {
    name: "Refuse rather than guess",
    text: "Invalid input is refused, never silently corrected. A value quietly clamped is a caller's bug that survives to matter later.",
  },
  {
    name: "Fail closed",
    text: "Every safety default is the restrictive one, and configuration that would relax a guarantee stops the process instead of lowering it.",
  },
  {
    name: "Make it structural",
    text: "A guarantee the type system holds beats one a runtime check holds, which beats one a comment asserts. Paper trading is held at the strongest of the three.",
  },
  {
    name: "Evidence, not assertion",
    text: "A claim about the system requires the output that proves it. A summary that omits a failure is a false statement, not an optimistic one.",
  },
  {
    name: "Bill what ran",
    text: "Costs and outcomes attribute to what actually executed, never to what was planned. Two claims about the same fact will disagree, and the louder one will be wrong.",
  },
] as const;

export default function CompanyPage() {
  return (
    <>
      <PageIntro
        eyebrow="Company"
        title="A research platform, run like one."
        lede="Algorik is a multi-regional AI and quantum research platform for investment decisions — and strictly a paper-trading one. It exists for the desk that needs to know not just what a system decided, but why, on what evidence, at what cost, and whether the reasoning held up."
      />

      <Section
        eyebrow="What we are"
        title="And what we are not"
        lede="Being precise about scope is part of the product."
      >
        <CardGrid columns={2}>
          <Card title="Algorik is">
            <p>
              A decision loop whose every step is reproducible and attributable after the fact:
              sensing, reasoning, simulation, deterministic risk checks and simulated execution,
              recorded in a hash-chained log.
            </p>
          </Card>
          <Card title="Algorik is not">
            <p>
              A brokerage, a trading venue, an investment adviser, or a system that submits live
              orders. No performance is promised, and simulated results are not presented as
              predictions of real trading.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        raised
        eyebrow="How we work"
        title="Operating principles"
        lede="These are the rules the codebase is actually held to, quoted from the way we build rather than written for this page."
      >
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {PRINCIPLES.map((principle) => (
            <div
              key={principle.name}
              className="rounded-[8px] border border-[color:var(--color-line)] bg-[color:var(--color-void)] p-5"
            >
              <h3 className="text-[13px] font-semibold uppercase tracking-[0.07em]">
                {principle.name}
              </h3>
              <p className="mt-2 text-[13px] leading-[1.6] text-[color:var(--color-ink-dim)]">
                {principle.text}
              </p>
            </div>
          ))}
        </div>
      </Section>

      <Section
        id="contact"
        eyebrow="Contact"
        title="Reach the team"
        lede="Institutional evaluations, security questions, integration interest, or a correction to anything this site claims — all welcome."
      >
        <div className="max-w-[560px] rounded-[14px] border border-[color:var(--color-line)] bg-[color:var(--color-surface)] p-6">
          <p className="text-[14px] leading-[1.7] text-[color:var(--color-ink-dim)]">
            Write to{" "}
            <a
              href="mailto:contact@algorik.ai"
              className="font-medium text-[color:var(--color-accent)] underline underline-offset-2"
            >
              contact@algorik.ai
            </a>
            .
          </p>
          <p className="mt-3 text-[12px] leading-[1.6] text-[color:var(--color-ink-faint)]">
            Placeholder address, pending domain setup — mail to it is not yet delivered. Until the
            domain is live, this page is honest about that rather than showing a form that goes
            nowhere.
          </p>
        </div>
      </Section>
    </>
  );
}
