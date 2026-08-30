import React from 'react'

export default function Process() {
  return (
    <>
      <section className="process-section">
            <div className="auto-container">
                <div className="inner-container pt_100 pb_70">
                    <div className="sec-title centred pb_60">
                        <span className="sub-title mb_14">The Process</span>
                        <h2>How It Works</h2>
                    </div>
                    <div className="row clearfix">
                        <div className="col-lg-6 col-md-12 col-sm-12 content-column">
                            <div className="content-box">
                                <div className="process-block-one">
                                    <div className="inner-box">
                                        <div className="shape" style={{ backgroundImage: "url(assets/images/shape/shape-3.png)" }}></div>
                                        <span className="count-text">1</span>
                                        <h3>Create your account</h3>
                                        <p>Sign up and land in a simulated desk — the full platform with zero capital at risk.</p>
                                    </div>
                                </div>
                                <div className="process-block-one">
                                    <div className="inner-box">
                                        <div className="shape" style={{ backgroundImage: "url(assets/images/shape/shape-3.png)" }}></div>
                                        <span className="count-text">2</span>
                                        <h3>Watch the loop reason</h3>
                                        <p>Opportunities, agent debate, risk refusals and paper executions stream to your dashboard as they happen.</p>
                                    </div>
                                </div>
                                <div className="process-block-one">
                                    <div className="inner-box">
                                        <div className="shape" style={{ backgroundImage: "url(assets/images/shape/shape-3.png)" }}></div>
                                        <span className="count-text">3</span>
                                        <h3>Audit every decision</h3>
                                        <p>Trace any outcome back through the hash-chained log to the evidence it stood on.</p>
                                    </div>
                                </div>
                            </div>
                        </div>
                        <div className="col-lg-6 col-md-12 col-sm-12 image-column">
                            <div className="image-box">
                                <figure className="image image-hov-two"><img src="assets/images/resource/process-1.jpg" alt=""/></figure>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
