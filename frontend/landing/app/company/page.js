import Layout from "@/components/layout/Layout"
import { Card, CardGrid, CtaBand, PostureNote, SectionTitle } from "@/components/sections/Blocks"
import Funfact from "@/components/sections/home1/Funfact"

export const metadata = {
    title: "Company",
    description:
        "What Algorik is — a paper-trading research platform — the operating principles it is built under, and what it deliberately is not.",
}

/**
 * The principles are the repository's real working rules, not marketing
 * inventions; each one names the failure it prevents. `/about` redirects here
 * so there is exactly one company page to keep true.
 */
const PRINCIPLES = [
    ["Say why", "Code, commits and decisions name the failure they prevent. A record that restates what happened without why is worse than none, because it reads as an answer."],
    ["Refuse rather than guess", "Invalid input is refused, never silently corrected. A value quietly clamped is a caller's bug that survives to matter later."],
    ["Fail closed", "Every safety default is the restrictive one, and configuration that would relax a guarantee stops the process instead of lowering it."],
    ["Make it structural", "A guarantee the type system holds beats one a runtime check holds, which beats one a comment asserts. Paper trading is held at the strongest of the three."],
    ["Evidence, not assertion", "A claim about the system requires the output that proves it. A summary that omits a failure is a false statement, not an optimistic one."],
    ["Bill what ran", "Costs and outcomes attribute to what actually executed, never to what was planned. Two claims about the same fact will disagree, and the louder one will be wrong."],
]

export default function CompanyPage() {
    return (
        <div className="boxed_wrapper">
            <Layout
                breadcrumbTitle="Company"
                breadcrumbLede="A research platform, run like one. Algorik exists for the desk that needs to know not just what a system decided, but why, on what evidence, at what cost, and whether the reasoning held up."
            >
                <section className="about-section pt_100 pb_100" id="what-we-are">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="What we are"
                            title="And what we are not"
                            lede="Being precise about scope is part of the product."
                        />
                        <CardGrid columns={2}>
                            <Card title="Algorik is">
                                <p>A decision loop whose every step is reproducible and attributable after the fact:
                                    sensing, reasoning, simulation, deterministic risk checks and simulated
                                    execution, recorded in a hash-chained log.</p>
                            </Card>
                            <Card title="Algorik is not">
                                <p>A brokerage, a trading venue, an investment adviser, or a system that submits live
                                    orders. No performance is promised, and simulated results are not presented as
                                    predictions of real trading.</p>
                            </Card>
                        </CardGrid>
                        <div className="pt_50">
                            <PostureNote>
                                Strictly paper trading. It never submits a live order — see the{" "}
                                <a href="/legal/risk-disclosures">risk disclosures</a> for what that means for
                                anything you read on this site.
                            </PostureNote>
                        </div>
                    </div>
                </section>

                <section className="process-section pt_0 pb_70" id="principles">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="How we work"
                            title="Operating principles"
                            lede="These are the rules the codebase is actually held to, quoted from the way we build rather than written for this page."
                        />
                        <CardGrid columns={3}>
                            {PRINCIPLES.map(([title, body]) => (
                                <Card key={title} title={title}><p>{body}</p></Card>
                            ))}
                        </CardGrid>
                    </div>
                </section>

                <Funfact />

                <CtaBand
                    title="A correction to anything this site claims is welcome"
                    lede="Institutional evaluations, security questions, integration interest — or a sentence here you can disprove."
                    secondaryHref="/contact"
                    secondaryLabel="Contact us"
                />
            </Layout>
        </div>
    )
}
