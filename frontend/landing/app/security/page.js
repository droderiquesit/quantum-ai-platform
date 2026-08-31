import Layout from "@/components/layout/Layout"
import { Card, CardGrid, CtaBand, Figure, PostureNote, SectionTitle } from "@/components/sections/Blocks"
import { BoundaryDiagram, ChainDiagram } from "@/components/art/Diagrams"

export const metadata = {
    title: "Security",
    description:
        "The paper-trading boundary held at three structural layers, a hash-chained audit log, keyless workload identity, and secrets that never touch an environment variable.",
}

export default function SecurityPage() {
    return (
        <div className="boxed_wrapper">
            <Layout
                breadcrumbTitle="Security"
                breadcrumbLede="Safety that is structural, not configurable. The properties that matter most are not settings someone remembered to enable."
            >
                <section className="about-section pt_100 pb_100" id="boundary">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="The boundary"
                            title="Paper trading, enforced three times"
                            lede="Algorik never submits a live order. Three independent layers hold that line, and each one catches a different way the mistake could arrive."
                        />
                        <Figure caption="Infrastructure catches the reviewed, committed mistake. The composition roots catch the unreviewed configuration edit. The type system catches what neither saw.">
                            <BoundaryDiagram />
                        </Figure>
                        <div className="pt_50">
                            <CardGrid columns={3}>
                                <Card title="1 — Infrastructure as code">
                                    <p>The deployment configuration refuses any live autonomy ceiling at plan time,
                                        so a live value cannot reach a running cluster through a reviewed, committed
                                        change.</p>
                                </Card>
                                <Card title="2 — The composition roots">
                                    <p>Every service binary re-checks its ceiling at start-up. A live value stops the
                                        process — it is never silently lowered to paper. This catches the unreviewed,
                                        hand-edited configuration the first layer never saw.</p>
                                </Card>
                                <Card title="3 — The type system">
                                    <p>The regional execution cell has no constructor that accepts anything but a
                                        paper-trading ceiling, and deterministic risk checks return a type that cannot
                                        name a model. Some mistakes are made unrepresentable rather than merely
                                        rejected.</p>
                                </Card>
                            </CardGrid>
                        </div>
                        <div className="pt_50">
                            <PostureNote>
                                The venue credential is readable only where the ceiling could use it, and every
                                surface renders the paper trading label wherever posture is shown — including
                                this one.
                            </PostureNote>
                        </div>
                    </div>
                </section>

                <section className="about-section pt_0 pb_100" id="audit">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Audit"
                            title="A log that cannot quietly change"
                            lede="Every event links cryptographically to the one before it. The chain is the record of the platform — and the point of a chain is what it refuses."
                        />
                        <div className="algorik-split">
                            <Figure caption="Nothing can write a record that cannot be replayed, and nothing can edit history that has been sealed.">
                                <ChainDiagram />
                            </Figure>
                            <CardGrid columns={1}>
                                <Card title="Hash-chained events">
                                    <p>A decision, its evidence, its cost and its outcome are all reproducible from
                                        the log alone.</p>
                                </Card>
                                <Card title="Attributed control">
                                    <p>Autonomy changes pass through one controller and carry an authenticated
                                        operator identity. The kill switch halts trading in scopes, requires a typed
                                        reason, and records who acted and why — and a halt cannot be cleared by the
                                        surface that did not cause it.</p>
                                </Card>
                            </CardGrid>
                        </div>
                    </div>
                </section>

                <section className="about-section pt_0 pb_100" id="secrets">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Secrets"
                            title="No keys to steal"
                            lede="The cheapest credential to protect is the one that never exists."
                        />
                        <CardGrid columns={3}>
                            <Card title="Keyless workload identity">
                                <p>Workloads authenticate to the cloud through workload identity federation. No
                                    downloaded service-account keys exist — not in files, not in examples, not in CI.</p>
                            </Card>
                            <Card title="Secrets as files, never environment">
                                <p>Secret material reaches pods as mounted files, never as environment variables. A
                                    key in the environment is a key in every child process and every crash dump; a
                                    file is readable by exactly the process that needs it.</p>
                            </Card>
                            <Card title="Nothing secret in the browser">
                                <p>This site and the portal receive nothing the public may not see. The session
                                    cookie carries an identifier and a signature — no token, no role, no entitlement
                                    — and everything it means is decided server-side.</p>
                            </Card>
                        </CardGrid>
                    </div>
                </section>

                <section className="about-section pt_0 pb_100" id="scope">
                    <div className="auto-container">
                        <SectionTitle eyebrow="Scope" title="What we claim, and what we do not" />
                        <Card title="No certification is claimed">
                            <p>Independent audits and certifications are not yet claimed; controls are implemented
                                and testable in the repository. That is the standard every statement on this page is
                                written to: describable, inspectable, and checkable by a person rather than asserted
                                by the system about itself.</p>
                        </Card>
                    </div>
                </section>

                <CtaBand
                    title="Security questions welcome"
                    lede="A research desk evaluating Algorik gets the same answer the codebase gives: the controls, named, and where they are held."
                    secondaryHref="/contact"
                    secondaryLabel="Contact us"
                />
            </Layout>
        </div>
    )
}
