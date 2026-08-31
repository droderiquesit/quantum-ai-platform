import { ChainDiagram } from "@/components/art/Diagrams"

/**
 * How it works. The template put `process-1.jpg` in the right-hand column —
 * another grey placeholder. The chain diagram replaces it because the third
 * step is the claim the picture should be making.
 */
const STEPS = [
    ["Create your account", "Sign up and land in a simulated desk — the full platform with zero capital at risk."],
    ["Watch the loop reason", "Opportunities, agent debate, risk refusals and paper executions stream to your dashboard as they happen."],
    ["Audit every decision", "Trace any outcome back through the hash-chained log to the evidence it stood on."],
]

export default function Process() {
    return (
        <section className="process-section" id="how-it-works">
            <div className="auto-container">
                <div className="inner-container pt_100 pb_70">
                    <div className="sec-title centred pb_60">
                        <span className="sub-title mb_14">The process</span>
                        <h2>How it works</h2>
                    </div>
                    <div className="row clearfix align-items-center">
                        <div className="col-lg-6 col-md-12 col-sm-12 content-column">
                            <div className="content-box">
                                {STEPS.map(([title, body], index) => (
                                    <div key={title} className="process-block-one">
                                        <div className="inner-box">
                                            <div className="shape" style={{ backgroundImage: "url(/assets/images/shape/shape-3.png)" }}></div>
                                            <span className="count-text">{index + 1}</span>
                                            <h3>{title}</h3>
                                            <p>{body}</p>
                                        </div>
                                    </div>
                                ))}
                            </div>
                        </div>
                        <div className="col-lg-6 col-md-12 col-sm-12 image-column">
                            <figure className="algorik-figure">
                                <ChainDiagram />
                                <figcaption>
                                    Each record carries the digest of the one before it, so history that has
                                    been sealed cannot be quietly edited.
                                </figcaption>
                            </figure>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    )
}
