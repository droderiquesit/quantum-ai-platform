import Layout from "@/components/layout/Layout"
import { Card, CardGrid, CtaBand, Figure, NumberedList, PostureNote, SectionTitle } from "@/components/sections/Blocks"
import { FunnelDiagram, LoopDiagram } from "@/components/art/Diagrams"
import Trading from "@/components/sections/home1/Trading"

export const metadata = {
    title: "Platform",
    description:
        "What the Algorik platform serves today: an eight-stage loop, opportunities, strategies, capital envelopes, risk and kill switch, simulated execution — with honest notes on what is still in research.",
}

/**
 * The stage names are the kernel's own, not marketing inventions, and the
 * capability cards say "in research" where a capability has no HTTP surface
 * rather than implying one exists. Copy is kept word-for-word aligned with the
 * portal's marketing pages so the two surfaces cannot say different things.
 */
const STAGES = [
    ["Sense", "Market and reference data are absorbed with bounded retention, each fact stamped with the instant it became knowable."],
    ["Understand", "Normalised events become regimes, correlations and context the rest of the loop can reason over."],
    ["Discover", "Candidate opportunities are detected and scored; most are discarded, and the discard reason is recorded."],
    ["Reason", "A panel of specialist agents examines each surviving opportunity; every finding carries its evidence and a computed confidence."],
    ["Simulate", "Proposals are rehearsed against a simulator before anything is decided about them."],
    ["Decide", "Deterministic risk and capital checks accept or veto — and a veto is a recorded outcome, never a silent drop."],
    ["Act", "Accepted orders execute against simulated venues only. There is no live path."],
    ["Learn", "Outcomes are attributed back to the reasoning that produced them, and the scoring feeds the next cycle."],
]

export default function PlatformPage() {
    return (
        <div className="boxed_wrapper">
            <Layout
                breadcrumbTitle="Platform"
                breadcrumbLede="The portal shows what the platform did, not what it hopes. Every panel is backed by a typed REST route or a server-sent event stream."
            >
                <section className="process-section pt_100 pb_70" id="loop">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="The loop"
                            title="Eight stages, every cycle"
                            lede="Each stage reports what it produced and why — a cycle that traded and a cycle where nothing cleared the bar are equally legible afterwards."
                        />
                        <div className="algorik-split pb_60">
                            <Figure caption="The stage names are the kernel's own. Act is highlighted because it is the one that never reaches a live venue.">
                                <LoopDiagram />
                            </Figure>
                            <Figure caption="Most candidates never become a proposal, and the reason each one was dropped is part of the record.">
                                <FunnelDiagram />
                            </Figure>
                        </div>
                        <NumberedList items={STAGES} />
                    </div>
                </section>

                <section className="about-section pt_0 pb_100" id="capabilities">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Capabilities"
                            title="What is served today"
                            lede="Six areas make up the working surface. Each card names its real status: served over the API now, or in research."
                        />
                        <CardGrid columns={2}>
                            <Card title="Opportunities" meta="served today">
                                <p>The opportunity queue is a first-class surface. Each entry is a detected, scored
                                    edge with the evidence behind the score — and it is called an opportunity on
                                    every screen, never a signal on one and a recommendation on the next.</p>
                            </Card>
                            <Card title="Strategies & champion/challenger" meta="served today">
                                <p>A strategy is a named, versioned decision procedure that can hold capital.
                                    Candidates climb a promotion ladder rung by rung, and the ladder stage of every
                                    candidate is readable from the API. The desk that produces them runs in a
                                    process that structurally cannot reach a venue.</p>
                            </Card>
                            <Card title="Portfolio & capital envelopes" meta="served today">
                                <p>The book is summarised with its paper-only flag, and capital is allocated through
                                    time-bounded envelopes with explicit bounds and recalls — a cell trades inside
                                    its envelope, alone.</p>
                                <p>Position-level detail and a cash ledger are not yet served over HTTP; both are in
                                    research, and the console renders that fact rather than a placeholder chart.</p>
                            </Card>
                            <Card title="Risk & kill switch" meta="served today">
                                <p>Exposure, concentration and the limits bounding them are read from one surface,
                                    and limits are checked before an order object exists. The kill switch halts
                                    trading in scopes, carries an authenticated operator identity and a typed
                                    reason, and every halt and recovery is audited.</p>
                            </Card>
                            <Card title="Execution — simulated venues" meta="served today">
                                <p>Orders, refusals, reconciliation breaks and fills are all readable, and every
                                    fill records that it was simulated. The platform serves no write path for a live
                                    order: the API answers an order submission with a refusal, by design.</p>
                                <p>Operator-entered paper order submission over HTTP is in research.</p>
                            </Card>
                            <Card title="Data & telemetry" meta="partly in research">
                                <p>Data sources are catalogued with their licensing posture — evaluated before a
                                    source is used, not after — and five server-sent event streams carry market,
                                    signal, order, position and health updates to the console.</p>
                                <p>Per-source health and provenance fields, and platform-wide metrics
                                    instrumentation, are in research. Algorik does not describe itself as fully
                                    observable until they ship.</p>
                            </Card>
                        </CardGrid>
                    </div>
                </section>

                <Trading />

                <section className="about-section pt_0 pb_100" id="design">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Design"
                            title="One vocabulary, end to end"
                            lede="The same object has the same name on every screen, because a queue called signals here and recommendations there teaches a reader they are different things."
                        />
                        <CardGrid columns={3}>
                            <Card title="Typed, not scraped">
                                <p>Every figure the console shows came from a typed response. When a route answers
                                    empty, the console shows the emptiness and the route that produced it — it never
                                    invents a number to fill a panel.</p>
                            </Card>
                            <Card title="Refusals are answers">
                                <p>A refusal from the platform is rendered as what the platform said, not rewritten
                                    into an empty list. Telling a reader that the platform refused is information;
                                    hiding it is a bug.</p>
                            </Card>
                            <Card title="Paper, labelled">
                                <p>Posture is declared wherever it is shown. Every execution surface carries the
                                    paper trading label, and no control in the console implies a live path exists.</p>
                            </Card>
                        </CardGrid>
                        <div className="pt_50">
                            <PostureNote>
                                Everything above executes against simulators and sandboxes. No route on this
                                platform submits an order to a live venue, and no page on this site offers one.
                            </PostureNote>
                        </div>
                    </div>
                </section>

                <CtaBand
                    title="See the loop for yourself"
                    lede="The portal renders the real cycle: what was sensed, what was reasoned, what was vetoed, and what was simulated."
                    secondaryHref="/technology"
                    secondaryLabel="How it reasons"
                />
            </Layout>
        </div>
    )
}
