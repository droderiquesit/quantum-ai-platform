import type { Metadata } from "next";
import { Card, CardGrid, CtaBand, PageIntro, Section } from "../ui";

export const metadata: Metadata = {
  title: "Platform",
  description:
    "What the Algorik portal actually serves today: opportunities, strategies, capital envelopes, risk and kill switch, simulated execution, data and telemetry — with honest notes on what is still in research.",
};

/**
 * The capability map mirrors the real route table
 * (`backend/crates/apps/qip-api/src/routes.rs`, mirrored in the portal's endpoint
 * spec). Where a capability is not served over HTTP yet, the card says
 * "in research" rather than implying a surface that does not exist.
 */
export default function PlatformPage() {
  return (
    <>
      <PageIntro
        eyebrow="Platform"
        title="The portal shows what the platform did, not what it hopes."
        lede="Every panel in the Algorik console is backed by a typed REST route or a server-sent event stream. Where a capability has no HTTP surface yet, the console says so by name — and so does this page."
      />

      <Section
        eyebrow="Capabilities"
        title="What is served today"
        lede="Six areas make up the working surface. Each card names its real status: served over the API now, or in research."
      >
        <CardGrid columns={2}>
          <Card title="Opportunities" meta="served today">
            <p>
              The opportunity queue is a first-class surface. Each entry is a detected, scored
              edge with the evidence behind the score — and it is called an opportunity on every
              screen, never a signal on one and a recommendation on the next.
            </p>
          </Card>
          <Card title="Strategies &amp; champion/challenger" meta="served today">
            <p>
              A strategy is a named, versioned decision procedure that can hold capital. Candidates
              climb a promotion ladder rung by rung, and the ladder stage of every candidate is
              readable from the API. The champion/challenger desk that produces them runs in the
              deep brain — a process that structurally cannot reach a venue.
            </p>
          </Card>
          <Card title="Portfolio &amp; capital envelopes" meta="served today">
            <p>
              The book is summarised with its paper-only flag, and capital is allocated through
              time-bounded envelopes with explicit bounds and recalls — a cell trades inside its
              envelope, alone.
            </p>
            <p>
              Position-level detail and a cash ledger are not yet served over HTTP; both are in
              research, and the console renders that fact rather than a placeholder chart.
            </p>
          </Card>
          <Card title="Risk &amp; kill switch" meta="served today">
            <p>
              Exposure, concentration and the limits bounding them are read from one surface, and
              limits are checked before an order object exists. The kill switch halts trading in
              scopes, carries an authenticated operator identity and a typed reason, and every
              halt and recovery is audited.
            </p>
          </Card>
          <Card title="Execution — simulated venues" meta="served today">
            <p>
              Orders, refusals, reconciliation breaks and fills are all readable, and every fill
              records that it was simulated. The platform serves no write path for a live order:
              the API answers an order submission with a refusal, by design.
            </p>
            <p>Operator-entered paper order submission over HTTP is in research.</p>
          </Card>
          <Card title="Data &amp; telemetry" meta="partly in research">
            <p>
              Data sources are catalogued with their licensing posture — evaluated before a source
              is used, not after — and five server-sent event streams carry market, signal, order,
              position and health updates to the console.
            </p>
            <p>
              Per-source health and provenance fields, and platform-wide metrics instrumentation,
              are in research. Algorik does not describe itself as fully observable until they
              ship.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        raised
        eyebrow="Design"
        title="One vocabulary, end to end"
        lede="The same object has the same name on every screen, because a queue called signals here and recommendations there teaches a reader they are different things."
      >
        <CardGrid>
          <Card title="Typed, not scraped">
            <p>
              Every figure the console shows came from a typed response. When a route answers
              empty, the console shows the emptiness and the route that produced it — it never
              invents a number to fill a panel.
            </p>
          </Card>
          <Card title="Refusals are answers">
            <p>
              A 4xx from the platform is rendered as what the platform said, not rewritten into an
              empty list. Telling a reader that the platform refused is information; hiding it is
              a bug.
            </p>
          </Card>
          <Card title="Paper, labelled">
            <p>
              Posture is declared wherever it is shown. Every execution surface carries the paper
              trading label, and no control in the console implies a live path exists.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <CtaBand
        title="See the loop for yourself"
        lede="The portal renders the real cycle: what was sensed, what was reasoned, what was vetoed, and what was simulated."
        secondaryHref="/technology"
        secondaryLabel="How it reasons"
      />
    </>
  );
}
