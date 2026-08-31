import Layout from "@/components/layout/Layout"
import { Card, CardGrid, CtaBand, PostureNote, SectionTitle } from "@/components/sections/Blocks"
import { CONTACT_EMAIL } from "@/lib/site"

export const metadata = {
    title: "Contact",
    description: "How to reach the Algorik desk, and an honest account of which channels answer today.",
}

/**
 * Contact, without a form.
 *
 * The template shipped a contact form and a search box, both raw markup with
 * no handler — one still posted to `index-3.html`. A form that accepts a
 * message and silently discards it is worse than no form: the sender believes
 * they have been heard. This deployment has no verified inbound mail path, so
 * this page prints the address with that caveat attached and offers no submit
 * button at all. When delivery is proven, a form can be added and this comment
 * deleted — not before.
 */
export default function ContactPage() {
    return (
        <div className="boxed_wrapper">
            <Layout
                breadcrumbTitle="Contact"
                breadcrumbLede="One address, kept in one place, so it can only be right or wrong once."
            >
                <section className="contact-section pt_90 pb_100" id="channels">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Reach the desk"
                            title="Write to us"
                            lede="Institutional evaluations, security questions, integration interest, or a correction to anything this site claims — all welcome."
                        />

                        <div className="algorik-channel">
                            <h3>Email</h3>
                            <p>
                                <a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a>
                            </p>
                            <p className="algorik-caveat">
                                This address is a placeholder pending domain setup, and delivery to it has not
                                been verified. It is printed with that caveat rather than dressed up as a contact
                                form: a form that accepts a message and drops it is the failure this page exists
                                to avoid. There is deliberately no submit button on this site.
                            </p>
                        </div>

                        <div className="pt_50">
                            <CardGrid columns={3}>
                                <Card title="The desk">
                                    <p>Algorik is operated as a research and risk desk. Every question about a
                                        decision the platform made has an answer in the audit log, and the answer is
                                        reproducible from that log alone.</p>
                                </Card>
                                <Card title="Posture">
                                    <p>Paper trading, end to end. Simulated execution only — no control on this
                                        platform submits a live order, and nothing on this site could place one.</p>
                                </Card>
                                <Card title="No office to print">
                                    <p>This deployment has no public office or telephone number. The template shipped
                                        both as invented placeholders; a fabricated address on a contact page is the
                                        fastest way to teach a visitor that the rest of the site lies too.</p>
                                </Card>
                            </CardGrid>
                        </div>

                        <div className="pt_50">
                            <PostureNote>
                                Nothing you send here is treated as an instruction to the platform. Algorik acts
                                only on its own loop, against simulated venues.
                            </PostureNote>
                        </div>
                    </div>
                </section>

                <CtaBand
                    title="Or open an account and watch it run"
                    lede="The portal renders the real cycle against simulated venues, with the log that explains every step it took."
                    secondaryHref="/company"
                    secondaryLabel="About Algorik"
                />
            </Layout>
        </div>
    )
}
