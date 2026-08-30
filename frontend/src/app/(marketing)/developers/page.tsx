import type { Metadata } from "next";
import { Bullets, Card, CardGrid, CtaBand, PageIntro, Section } from "../ui";

export const metadata: Metadata = {
  title: "Developers",
  description:
    "Algorik's typed REST surface and server-sent event streams, the self-served OpenAPI document, and the honest status of external API access: feature-gated off by default.",
};

export default function DevelopersPage() {
  return (
    <>
      <PageIntro
        eyebrow="Developers"
        title="A typed surface that describes itself."
        lede="Everything the Algorik console renders arrives over a REST route or a server-sent event stream, and the platform publishes its own OpenAPI description generated from the same route table it serves. What you can read about the API is what the API is."
      />

      <Section
        eyebrow="The surface"
        title="REST and streams, today"
        lede="These exist now and drive the portal in production form — none of this is a roadmap item."
      >
        <CardGrid columns={2}>
          <Card title="Typed REST" meta="served today">
            <p>
              Read routes cover system status, portfolio, opportunities, proposals, orders, fills,
              agents, autonomy, regions, strategies, capital, risk, attribution, data sources,
              training and quantum runs. Each route declares the least role it requires, and a
              refusal names what to do instead.
            </p>
          </Card>
          <Card title="Server-sent events" meta="served today">
            <p>
              Five stream channels — market, signals, orders, positions and health — push updates
              over SSE. Plain HTTP, inspectable with nothing more exotic than curl.
            </p>
          </Card>
          <Card title="OpenAPI, self-served" meta="served today">
            <p>
              The platform serves an OpenAPI 3.1 document generated from its own route table, so
              the description cannot drift from the router: a route the document declares is a
              route the platform answers, and tests hold the two together.
            </p>
          </Card>
          <Card title="No write path for live orders" meta="structural">
            <p>
              The API serves no route that could submit a live order, and answers an order
              submission attempt with a refusal. This is the paper-trading boundary as a
              developer meets it: a method that is not there.
            </p>
          </Card>
        </CardGrid>
      </Section>

      <Section
        raised
        eyebrow="Access"
        title="External API access is off by default"
        lede="Honesty about status: the surfaces above exist, and access to them from outside the deployment is gated."
      >
        <div className="max-w-[700px] space-y-4 text-[14px] leading-[1.7] text-[color:var(--color-ink-dim)]">
          <p>
            API keys and developer documentation for external integrators sit behind the{" "}
            <code className="num">developerPortal</code> feature flag, whose default is off — as is
            every default on this platform whose failure mode would be an accidental capability.
            A deployment where configuration failed to load gets the safe answer, not the
            convenient one.
          </p>
          <div>
            <Bullets
              items={[
                "Flag defaults are declared in one file, with a stated reason each.",
                "An undeclared flag cannot be read at all, so a typo resolves to a visible error rather than a silently disabled feature.",
                "When external access opens, it will be announced here — not discovered in a changelog.",
              ]}
            />
          </div>
        </div>
      </Section>

      <CtaBand
        title="Interested in building against Algorik?"
        lede="Tell us what you would integrate. External access is gated today, and interest from real integrators is what prioritises opening it."
        primaryHref="/company#contact"
        primaryLabel="Contact us"
        secondaryHref="/platform"
        secondaryLabel="See the platform"
      />
    </>
  );
}
