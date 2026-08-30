import Layout from "@/components/layout/Layout"
import Trading from "@/components/sections/home1/Trading"
import Process from "@/components/sections/home1/Process"
import Funfact from "@/components/sections/home1/Funfact"

/**
 * The platform page: what the loop is, in the template's own section
 * rhythm. Everything stated here is checkable against the repository —
 * the stages, the safeguards, the audit trail. Marketing that cannot be
 * audited is the one kind this platform must not ship.
 */
const STAGES = [
    ["Sense", "Market data, chain state and reference facts absorbed with the instant they became knowable."],
    ["Understand", "A bitemporal world model that can answer what was known, and when."],
    ["Discover", "Detectors scan for anomalies and opportunities, and say which detector fired."],
    ["Reason", "A panel of AI agents argues the thesis — including an adversary paid to break it."],
    ["Simulate", "Backtests and scenario runs against history before a unit of paper capital moves."],
    ["Decide", "Position sizing under hard limits that are checked before an order object exists."],
    ["Act", "Execution against simulators only. There is no live path to ease, enable, or misuse."],
    ["Learn", "Attribution scores the reasoning afterwards, and the loop keeps what survived."],
]

export default function Platform() {
    return (
        <div className="boxed_wrapper">
            <Layout headerStyle={1} footerStyle={1} breadcrumbTitle="Platform">

                <section className="process-section pt_100 pb_70">
                    <div className="auto-container">
                        <div className="sec-title centred pb_60">
                            <span className="sub-title mb_14">The loop</span>
                            <h2>Eight stages, every cycle</h2>
                        </div>
                        <div className="row clearfix">
                            {STAGES.map(([title, body], index) => (
                                <div key={title} className="col-lg-6 col-md-6 col-sm-12 content-column">
                                    <div className="process-block-one">
                                        <div className="inner-box">
                                            <span className="count-text">{index + 1}</span>
                                            <h3>{title}</h3>
                                            <p>{body}</p>
                                        </div>
                                    </div>
                                </div>
                            ))}
                        </div>
                    </div>
                </section>

                <Trading />

                <section className="about-section pt_0 pb_100">
                    <div className="auto-container">
                        <div className="sec-title centred pb_60">
                            <span className="sub-title mb_14">Trust, structurally</span>
                            <h2>Why paper trading holds</h2>
                        </div>
                        <div className="row clearfix">
                            <div className="col-lg-4 col-md-6 col-sm-12">
                                <div className="trading-block-one"><div className="inner-box">
                                    <h3>Infrastructure refuses</h3>
                                    <p>The deployment tooling rejects any live-trading configuration at plan time — a live ceiling never reaches a running process.</p>
                                </div></div>
                            </div>
                            <div className="col-lg-4 col-md-6 col-sm-12">
                                <div className="trading-block-one"><div className="inner-box">
                                    <h3>Start-up refuses</h3>
                                    <p>Every binary validates its autonomy ceiling as it boots. A live value stops the process — it is never silently lowered.</p>
                                </div></div>
                            </div>
                            <div className="col-lg-4 col-md-6 col-sm-12">
                                <div className="trading-block-one"><div className="inner-box">
                                    <h3>The types refuse</h3>
                                    <p>The execution cell has no constructor that accepts a live ceiling. The compiler holds the boundary a config file cannot.</p>
                                </div></div>
                            </div>
                        </div>
                    </div>
                </section>

                <Process />
                <Funfact />
            </Layout>
        </div>
    )
}
