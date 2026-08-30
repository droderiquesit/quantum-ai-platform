import type { Metadata } from "next";
import { Card, CardGrid, CtaBand, PageIntro, Section } from "../ui";

export const metadata: Metadata = {
  title: "Security",
  description:
    "The paper-trading boundary held at three structural layers, a hash-chained audit log, keyless workload identity, and secrets that never touch an environment variable.",
};

export default function SecurityPage() {
  return (
    <>
      <PageIntro
        eyebrow="Security"
        title="Safety that is structural, not configurable."
        lede="The properties that matter most on Algorik are not settings someone remembered to enable. They are held by infrastructure, by process start-up, and by the type system — three layers that fail closed, independently."
      />

      <Section
        id="boundary"
        eyebrow="The boundary"
        title="Paper trading, enforced three times"
        lede="Algorik never submits a live order. Three independent layers hold that line, and each one catches a different way the mistake could arrive."
      >
        <CardGrid>
          <Card title="1 — Infrastructure as code">
            <p>
              The deployment configuration refuses any live autonomy ceiling at plan time, so a
              live value cannot reach a running cluster through a reviewed, committed change.
            </p>
          </Card>
          <Card title="2 — The composition roots">
            <p>
              Every service binary re-checks its ceiling at start-up. A live value stops the
              process — it is never silently lowered to paper. This catches the unreviewed,
              hand-edited configuration the first layer never saw.
            </p>
          </Card>
          <Card title="3 — The type system">
            <p>
              The regional execution cell has no constructor that accepts anything but a
              paper-trading ceiling, and deterministic risk checks return a type that cannot name
              a model. Some mistakes are made unrepresentable rather than merely rejected.
            </p>
          </Card>
        </CardGrid>
        <p className="mt-6 max-w-[700px] text-[13px] leading-[1.65] text-[color:var(--color-ink-faint)]">
          The venue credential is readable only where the ceiling could use it, and the interface
          renders the paper trading label wherever posture is shown.
        </p>
      </Section>

      <Section
        id="audit"
        raised
        eyebrow="Audit"
        title="A log that cannot quietly change"
        lede="Every event links cryptographically to the one before it. The chain is the record of the platform — and the point of a chain is what it refuses."
      >
        <CardGrid columns={2}>
          <Card title="Hash-chained events">
            <p>
              Nothing can write a record that cannot be replayed, and nothing can edit history
              that has been sealed. A decision, its evidence, its cost and its outcome are all
              reproducible from the log alone.
            </p>
          </Card>
          <Card title="Attributed control">
            <p>
              Autonomy changes pass through one controller and carry an authenticated operator
              identity. The kill switch halts trading in scopes, requires a typed reason, and
              records who acted and why — and a halt cannot be cleared by the surface that did not
              cause it.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        eyebrow="Secrets"
        title="No keys to steal"
        lede="The cheapest credential to protect is the one that never exists."
      >
        <CardGrid>
          <Card title="Keyless workload identity">
            <p>
              Workloads authenticate to the cloud through workload identity federation. No
              downloaded service-account keys exist — not in files, not in examples, not in CI.
            </p>
          </Card>
          <Card title="Secrets as files, never environment">
            <p>
              Secret material reaches pods as mounted files, never as environment variables. A key
              in the environment is a key in every child process and every crash dump; a file is
              readable by exactly the process that needs it.
            </p>
          </Card>
          <Card title="Nothing secret in the browser">
            <p>
              The public site and the portal receive nothing the public may not see. The session
              cookie carries an identifier and a signature — no token, no role, no entitlement —
              and everything it means is decided server-side.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section eyebrow="Scope" title="What we claim, and what we do not">
        <div className="max-w-[700px] rounded-[8px] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-6 py-5">
          <p className="text-[14px] leading-[1.7] text-[color:var(--color-ink-dim)]">
            Independent audits and certifications are not yet claimed; controls are implemented
            and testable in the repository. That is the standard every statement on this page is
            written to: describable, inspectable, and checkable by a person rather than asserted
            by the system about itself.
          </p>
        </div>
      </Section>

      <CtaBand
        title="Security questions welcome"
        lede="A research desk evaluating Algorik gets the same answer the codebase gives: the controls, named, and where they are held."
        primaryHref="/company#contact"
        primaryLabel="Contact us"
        secondaryHref="/institutional"
        secondaryLabel="For institutions"
      />
    </>
  );
}
