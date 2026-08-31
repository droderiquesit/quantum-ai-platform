'use client'

import Link from "next/link"

/**
 * The client error boundary.
 *
 * `/error` used to be an ordinary route: a 404 page with a full navigation,
 * publicly linkable, returning HTTP 200 while claiming the page did not exist.
 * A crawler indexed it as a real page and a visitor could arrive at it from a
 * search result. It is a boundary now, which is what it always described.
 */
export default function GlobalError({ error, reset }) {
    return (
        <section className="error-section centred pt_160 pb_160">
            <div className="auto-container">
                <div className="content-box p_relative pt_200">
                    <div className="shape" style={{ backgroundImage: "url(/assets/images/icons/error-1.png)" }}></div>
                    <h1>Something broke</h1>
                    <p>
                        This page failed to render. Nothing on Algorik trades in response to a
                        browser error — the platform is paper trading and the console only reads.
                        {error?.digest ? <><br />Reference: {error.digest}</> : null}
                    </p>
                    <div className="algorik-cta-actions" style={{ justifyContent: "center", marginTop: 24 }}>
                        <button type="button" className="theme-btn btn-one" onClick={() => reset()}>Try again</button>
                        <Link href="/" className="theme-btn btn-two">Back to home</Link>
                    </div>
                </div>
            </div>
        </section>
    )
}
