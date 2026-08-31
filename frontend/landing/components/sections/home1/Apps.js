import { ConsoleMockup } from "@/components/art/Diagrams"
import { SIGN_UP } from "@/lib/site"

/**
 * The installed app. `mockup-1.png` was a 552x440 grey placeholder; the drawing
 * shows the console's real shape — including the PAPER TRADING label, which the
 * platform's rules require wherever posture is shown.
 */
export default function Apps() {
    return (
        <section className="apps-section">
            <div className="auto-container">
                <div className="inner-container">
                    <div className="shape" style={{ backgroundImage: "url(/assets/images/shape/shape-4.png)" }}></div>
                    <figure className="image-layer" style={{ width: 300 }}><ConsoleMockup /></figure>
                    <div className="content_block_two">
                        <div className="content-box">
                            <div className="sec-title light pb_40">
                                <span className="sub-title mb_14">The app</span>
                                <h2>Take Algorik with you</h2>
                                <p>Install the portal on any phone straight from the browser — same design, same
                                    session, same paper-trading guarantees. There is no app-store listing to
                                    send you to, because the phone app is the portal itself. Offline it shows
                                    nothing rather than a stale book.</p>
                            </div>
                            <div className="btn-box"><a href={SIGN_UP} className="theme-btn btn-one">Open the portal</a></div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    )
}
