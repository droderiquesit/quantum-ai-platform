import Link from "next/link"

const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400"

export default function Apps() {
  return (
    <>
      <section className="apps-section">
            <div className="auto-container">
                <div className="inner-container">
                    <div className="shape" style={{ backgroundImage: "url(assets/images/shape/shape-4.png)" }}></div>
                    <figure className="image-layer"><img src="assets/images/resource/mockup-1.png" alt=""/></figure>
                    <div className="content_block_two">
                        <div className="content-box">
                            <div className="sec-title light pb_40">
                                <span className="sub-title mb_14">The app</span>
                                <h2>Take Algorik with you</h2>
                                <p>Install the portal on any phone straight from the browser — same design, same session, same paper-trading guarantees. Offline it shows nothing rather than a stale book.</p>
                            </div>
                            <div className="btn-box"><a href={`${PORTAL}/sign-up`} className="theme-btn btn-one">Open the portal</a></div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
