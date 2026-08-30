import CounterUp from "@/components/elements/CounterUp"

export default function Funfact() {
  return (
    <>
      <section className="funfact-section">
            <div className="auto-container">
                <div className="inner-container">
                    <div className="row clearfix">
                        <div className="col-lg-4 col-md-6 col-sm-12 funfact-block">
                            <div className="funfact-block-one">
                                <div className="inner-box">
                                    <div className="count-outer">
                                        <CounterUp end={8} /><span className="text">Loop stages</span>
                                    </div>
                                    <p>Sense to learn — every cycle traverses all eight, and reports what each stage produced and refused.</p>
                                </div>
                            </div>
                        </div>
                        <div className="col-lg-4 col-md-6 col-sm-12 funfact-block">
                            <div className="funfact-block-one">
                                <div className="inner-box">
                                    <div className="count-outer">
                                        <CounterUp end={3} /><span className="text">Paper-trading safeguards</span>
                                    </div>
                                    <p>Infrastructure, process start-up, and the type system each refuse a live order independently of the other two.</p>
                                </div>
                            </div>
                        </div>
                        <div className="col-lg-4 col-md-6 col-sm-12 funfact-block">
                            <div className="funfact-block-one">
                                <div className="inner-box">
                                    <div className="count-outer">
                                        <CounterUp end={59} /><span className="text">Rust crates</span>
                                    </div>
                                    <p>One workspace, two runtime dependencies, zero unsafe blocks — an audit surface a person can actually read.</p>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
