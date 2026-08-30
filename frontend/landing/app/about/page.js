import Layout from "@/components/layout/Layout"
import About from "@/components/sections/home1/About"
import Funfact from "@/components/sections/home1/Funfact"
import Subscribe from "@/components/sections/home1/Subscribe"

/**
 * The company page. Algorik has principles before it has customers, so the
 * page states the principles — each one enforced somewhere in the codebase,
 * not aspirational copy.
 */
const PRINCIPLES = [
    ["Say why", "Every decision names the evidence it stood on, at what cost, and whether the reasoning held up afterwards."],
    ["Refuse rather than guess", "Invalid inputs are rejected, never silently corrected. A value quietly clamped is a bug that survives."],
    ["Fail closed", "Every safety default is the restrictive one. A configuration that would relax one stops the process instead."],
    ["A baseline, always", "Quantum methods run only where a measured benefit is demonstrable against the classical baseline computed every time."],
]

export default function AboutPage() {
    return (
        <div className="boxed_wrapper">
            <Layout headerStyle={1} footerStyle={1} breadcrumbTitle="Company">
                <About />

                <section className="process-section pt_0 pb_70">
                    <div className="auto-container">
                        <div className="sec-title centred pb_60">
                            <span className="sub-title mb_14">Principles</span>
                            <h2>Settled, and enforced in code</h2>
                        </div>
                        <div className="row clearfix">
                            {PRINCIPLES.map(([title, body], index) => (
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

                <Funfact />
                <Subscribe />
            </Layout>
        </div>
    )
}
