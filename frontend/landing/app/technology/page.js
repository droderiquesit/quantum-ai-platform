import Layout from "@/components/layout/Layout"
import { Bullets, Card, CardGrid, CtaBand, Figure, SectionTitle } from "@/components/sections/Blocks"
import { BaselineDiagram, PanelDiagram } from "@/components/art/Diagrams"

export const metadata = {
    title: "Technology",
    description:
        "How Algorik reasons: a seventeen-agent panel with computed confidence, and quantum experiments kept only where they measurably improve on a classical baseline.",
}

/** The reasoning roster, as the deep brain actually hosts it. */
const PANEL_AGENTS = [
    "Chief investment intelligence", "Macro analyst", "Equity analyst", "Credit analyst",
    "Microstructure analyst", "Derivatives analyst", "Commodities analyst", "FX and rates analyst",
    "Alternative data analyst", "Causal analyst", "Adversarial reviewer", "Simulation analyst",
    "Portfolio construction", "Quantum optimization", "Risk control", "Learning attribution",
    "Compliance control",
]

export default function TechnologyPage() {
    return (
        <div className="boxed_wrapper">
            <Layout
                breadcrumbTitle="Technology"
                breadcrumbLede="Reasoning you can check, not reasoning you must trust. Every conclusion carries its evidence, its computed confidence and its cost."
            >
                <section className="about-section pt_100 pb_100" id="ai">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="AI intelligence"
                            title="A seventeen-agent reasoning panel"
                            lede="The deep brain hosts seventeen specialist agents and refuses to host the one agent that touches a venue — reasoning and execution are separated by construction, not by convention."
                        />
                        <div className="algorik-split">
                            <div>
                                <p>Each agent examines an opportunity from its own discipline and returns findings
                                    with evidence attached. One agent failing does not silence the rest: dispatch is
                                    isolated per agent, because an organisation where one analyst&rsquo;s bug mutes
                                    the others is worse than one where a finding is missing.</p>
                                <p>The adversarial reviewer exists to disagree. A panel that always concurs is not a
                                    panel; it is one opinion with sixteen echoes.</p>
                                <p><strong>Confidence is arithmetic.</strong> A confidence figure is derived from the
                                    evidence by stated combination rules, so two screens can never disagree about how
                                    sure the platform is, and &ldquo;high conviction&rdquo; is never an adjective
                                    someone typed.</p>
                                <p>Deterministic pre-trade risk checks never route to a model. The cost router makes
                                    that structural: where determinism is required, the routing type cannot even name
                                    a model rung.</p>
                            </div>
                            <Figure caption="The marked seat is the adversarial reviewer. The absent seat is execution — the node that reasons refuses, at start-up, to host anything that could reach a venue.">
                                <PanelDiagram />
                            </Figure>
                        </div>
                        <div className="pt_50">
                            <Card title="The panel">
                                <ul className="algorik-bullets" style={{ columnCount: 2, columnGap: 40 }}>
                                    {PANEL_AGENTS.map((agent) => <li key={agent}>{agent}</li>)}
                                </ul>
                            </Card>
                        </div>
                    </div>
                </section>

                <section className="about-section pt_0 pb_100" id="quantum">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Quantum research"
                            title="A classical baseline, every time"
                            lede="Algorik runs QAOA experiments for portfolio optimisation on a hosted quantum runtime, with a local steepest-descent fallback. The differentiator is the rule the experiments run under."
                        />
                        <div className="algorik-split">
                            <Figure caption="Both halves are computed on the same problem, at the same time, so the comparison is a thing a person can check rather than a thing the platform asserts about itself.">
                                <BaselineDiagram />
                            </Figure>
                            <div>
                                <p>Every quantum job is paired with a classical baseline computed at the same time,
                                    on the same problem. Quantum methods stay in the loop only where a measured
                                    benefit against that baseline is demonstrable — and are removed where it is not.</p>
                                <p>The comparison itself is readable from the API: quantum jobs and their classical
                                    baselines are served side by side.</p>
                                <h3>What that rules out</h3>
                                <Bullets items={[
                                    "Reporting a quantum run without its classical twin.",
                                    "Keeping a quantum path because it is novel rather than because it measured better.",
                                    "Describing hardware used as if it were performance achieved.",
                                ]} />
                            </div>
                        </div>
                    </div>
                </section>

                <section className="about-section pt_0 pb_100" id="engineering">
                    <div className="auto-container">
                        <SectionTitle eyebrow="Engineering" title="Deliberately boring where it counts" />
                        <CardGrid columns={3}>
                            <Card title="Rust, end to end">
                                <p>Every latency-sensitive and core service is Rust, with blocking I/O and an
                                    explicit timeout on every call that leaves the process. The browser layer is the
                                    one deliberate exception.</p>
                            </Card>
                            <Card title="A tiny supply chain">
                                <p>The core platform admits two third-party libraries, total. Every additional
                                    dependency is an architecture decision with a written record, because a
                                    dependency is an audit surface, not a download.</p>
                            </Card>
                            <Card title="Determinism where money moves">
                                <p>Money is decimal arithmetic, iteration orders are stable, and a replay of the log
                                    is a replay — not an approximation that usually agrees.</p>
                            </Card>
                        </CardGrid>
                    </div>
                </section>

                <CtaBand
                    title="Read the reasoning, not a summary of it"
                    lede="Findings, confidence arithmetic and baseline comparisons are all part of the record the platform keeps."
                    secondaryHref="/security"
                    secondaryLabel="How it is secured"
                />
            </Layout>
        </div>
    )
}
