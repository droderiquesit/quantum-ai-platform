import Link from "next/link"

/** The real 404 — reached by Next when a route does not exist, and answering
 *  with HTTP 404 rather than a 200 that says "not found" in prose. */
export default function NotFound() {
    return (
        <section className="error-section centred pt_160 pb_160">
            <div className="auto-container">
                <div className="content-box p_relative pt_200">
                    <div className="shape" style={{ backgroundImage: "url(/assets/images/icons/error-1.png)" }}></div>
                    <h1>404</h1>
                    <p>This page does not exist, or it was removed.<br />The site map is in the footer of every other page.</p>
                    <div className="algorik-cta-actions" style={{ justifyContent: "center", marginTop: 24 }}>
                        <Link href="/" className="theme-btn btn-one">Back to home</Link>
                        <Link href="/platform" className="theme-btn btn-two">See the platform</Link>
                    </div>
                </div>
            </div>
        </section>
    )
}
