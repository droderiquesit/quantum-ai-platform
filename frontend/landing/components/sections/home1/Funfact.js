import CounterUp from "@/components/elements/CounterUp"
import { Claim } from "@/components/elements/Claim"

/**
 * The three figures a visitor is most likely to remember, and therefore the
 * three most worth being exact about.
 *
 * Every one of them is a property of the repository — the kernel's `Stage`
 * enum has eight variants, three independent layers refuse a live order, the
 * workspace has a fixed number of members — and none of them is an observation
 * of a running system, because nothing is deployed. They are annotated
 * `architecture` for that reason, and a strip of large animated numbers with
 * no status on them is exactly the surface §40.6 was written about.
 *
 * The crate count read **59** until this sweep, and the workspace has 58
 * members (`grep -c '"crates/' backend/Cargo.toml`). Nobody introduced that
 * gap deliberately; a crate was removed and the marketing figure was not a
 * thing anyone thought to recount, which is precisely how a number on a public
 * page comes to be wrong. The annotation does not make the figure true — only
 * counting does — but it names where to go and check, and it is the reason
 * this one was counted at all.
 */
const FIGURES = [
    [8, "Loop stages", "Sense to learn — every cycle traverses all eight, and reports what each stage produced and refused."],
    [3, "Paper-trading safeguards", "Infrastructure, process start-up, and the type system each refuse a live order independently of the other two."],
    [58, "Rust crates", "One workspace, two runtime dependencies, zero unsafe blocks — an audit surface a person can actually read."],
]

export default function Funfact() {
  return (
    <>
      <section className="funfact-section">
            <div className="auto-container">
                <div className="inner-container">
                    <div className="row clearfix">
                        {FIGURES.map(([count, label, body]) => (
                            <div key={label} className="col-lg-4 col-md-6 col-sm-12 funfact-block">
                                <div className="funfact-block-one">
                                    <div className="inner-box">
                                        <div className="count-outer">
                                            <Claim status="architecture">
                                                <CounterUp end={count} /><span className="text">{label}</span>
                                            </Claim>
                                        </div>
                                        <Claim status="architecture" as="p">{body}</Claim>
                                    </div>
                                </div>
                            </div>
                        ))}
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
