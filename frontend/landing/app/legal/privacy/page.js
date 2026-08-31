import Link from "next/link"
import Layout from "@/components/layout/Layout"
import { Bullets, LegalSection, LegalShell } from "@/components/sections/Blocks"

export const metadata = {
    title: "Privacy policy",
    description:
        "Draft privacy policy for Algorik, describing what data the platform collects, why, and for how long. Not yet reviewed by counsel and not in effect.",
}

export default function PrivacyPage() {
    return (
        <div className="boxed_wrapper">
            <Layout breadcrumbTitle="Privacy policy">
                <LegalShell title="Privacy Policy">
                    <LegalSection heading="1. Scope">
                        <p>This policy will describe how Algorik handles personal data across the public site and
                            the signed-in platform. It is written to say what actually happens, in the order a
                            reader would ask: what is collected, why, where it goes, and how long it is kept.</p>
                    </LegalSection>

                    <LegalSection heading="2. What we collect">
                        <Bullets items={[
                            "Account data: email address, optional display name, account type, and the agreements you have accepted.",
                            "Session data: a signed session identifier in a cookie, sign-in method, and authentication timestamps.",
                            "Audit records: actions taken on the platform, with the operator identity and stated reason where one is required. These are part of the platform's tamper-evident log.",
                            "Operational logs: request metadata needed to run and secure the Service.",
                        ]} />
                        <p>We do not collect payment card data, government identifiers, or brokerage account
                            credentials — the Service is paper trading and has no use for them.</p>
                    </LegalSection>

                    <LegalSection heading="3. What we use it for">
                        <Bullets items={[
                            "Operating the Service: authentication, authorisation and support.",
                            "Security: detecting abuse and investigating incidents.",
                            "Auditability: the platform's purpose includes an attributable record of decisions and operator actions.",
                        ]} />
                        <p>We do not sell personal data, and this public site sets no third-party advertising or
                            tracking cookies.</p>
                    </LegalSection>

                    <LegalSection heading="4. Cookies and local storage">
                        <Bullets items={[
                            "This public site sets no cookies of its own. It embeds no analytics and no third-party scripts.",
                            "A session cookie and a request-forgery-protection cookie, both scoped to the portal's origin, are used for signed-in access there.",
                            "A theme preference may be stored in your browser's local storage by the portal. It never leaves your browser.",
                        ]} />
                    </LegalSection>

                    <LegalSection heading="5. Retention">
                        <p>Working data is kept under explicit bounds, and audit records are retained for as long
                            as the record they attest to is retained — a tamper-evident log that could be
                            selectively shortened would not be one. Account data is deleted or anonymised on
                            verified request, except where a record must be preserved for security or legal
                            reasons; where that applies, we will say so in the response.</p>
                    </LegalSection>

                    <LegalSection heading="6. Sharing">
                        <p>Personal data is shared only with infrastructure providers processing it on our behalf
                            under contract, and where the law requires disclosure. A future change to this list
                            takes effect only through a published revision of this policy.</p>
                    </LegalSection>

                    <LegalSection heading="7. Your rights">
                        <p>Depending on your jurisdiction, you may have rights to access, correct, export,
                            restrict or delete personal data. Requests can be made via the{" "}
                            <Link href="/contact">contact page</Link> and will be answered within the period your
                            jurisdiction requires.</p>
                    </LegalSection>

                    <LegalSection heading="8. Changes">
                        <p>Material changes will be published with a new effective date before they apply. This
                            draft has no effective date and is not yet in force.</p>
                    </LegalSection>
                </LegalShell>
            </Layout>
        </div>
    )
}
