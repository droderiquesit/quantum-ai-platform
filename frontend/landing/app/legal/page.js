import Link from "next/link"
import Layout from "@/components/layout/Layout"
import { Card, CardGrid, PostureNote, SectionTitle } from "@/components/sections/Blocks"

export const metadata = {
    title: "Legal & disclosures",
    description:
        "Algorik's terms of service, privacy policy and risk disclosures — all drafts, published so the intent is inspectable.",
}

const DOCUMENTS = [
    ["/legal/risk-disclosures", "Risk disclosures",
        "What simulated results can and cannot tell you, the limitations of simulation, and the plain statement that this platform does not execute live trades."],
    ["/legal/terms", "Terms of service",
        "What using Algorik would mean: a simulated service, no investment advice, no brokerage, and the acceptable use that includes not circumventing the paper-trading boundary."],
    ["/legal/privacy", "Privacy policy",
        "What personal data the platform collects, why, who it is shared with, and how long it is kept — including why audit records cannot be selectively shortened."],
]

export default function LegalIndex() {
    return (
        <div className="boxed_wrapper">
            <Layout
                breadcrumbTitle="Legal & disclosures"
                breadcrumbLede="Three documents. All three are drafts that have not been reviewed by counsel, and each says so at the top of itself."
            >
                <section className="about-section pt_100 pb_100">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="Disclosures"
                            title="Read the risk disclosures first"
                            lede="It is the document that governs how everything else on this site should be read."
                        />
                        <CardGrid columns={3}>
                            {DOCUMENTS.map(([href, title, body]) => (
                                <Card key={href} title={<Link href={href}>{title}</Link>}>
                                    <p>{body}</p>
                                    <p><Link href={href} className="theme-btn btn-two">Read it</Link></p>
                                </Card>
                            ))}
                        </CardGrid>
                        <div className="pt_50">
                            <PostureNote>
                                Algorik is a paper-trading research platform. It submits no orders to any live
                                venue. Should that ever change, the risk disclosures would change first, with a
                                new effective date and prominent notice — not as a quiet edit.
                            </PostureNote>
                        </div>
                    </div>
                </section>
            </Layout>
        </div>
    )
}
