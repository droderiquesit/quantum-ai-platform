import Link from "next/link"

export default function Experience() {
  return (
    <>
        <section className="experience-section">
            <div className="auto-container">
                <div className="inner-container">
                    <div className="shape" style={{ backgroundImage: "url(assets/images/shape/shape-19.png)" }}></div>
                    <div className="row clearfix">
                        <div className="col-lg-6 col-md-12 col-sm-12 content-column">
                            <div className="content-box">
                                <div className="sec-title light pb_14">
                                    <span className="sub-title mb_14">Experience</span>
                                    <h2>Your trades in the right place</h2>
                                </div>
                                <div className="text-box">
                                    <p>We use cookines to understand how you use our website and to give you the best possible</p>
                                    <Link href="/index-3" className="theme-btn btn-one">Create Account</Link>
                                </div>
                            </div>
                        </div>
                        <div className="col-lg-6 col-md-12 col-sm-12 inner-column">
                            <div className="inner-box">
                                <div className="row clearfix">
                                    <div className="col-lg-6 col-md-6 col-sm-12 single-column">
                                        <div className="single-item mb_95">
                                            <h2>1 milion+</h2>
                                            <p>XTB Group Clients</p>
                                        </div>
                                    </div>
                                    <div className="col-lg-6 col-md-6 col-sm-12 single-column">
                                        <div className="single-item mb_95">
                                            <h2>5 milion+</h2>
                                            <p>App downloads</p>
                                        </div>
                                    </div>
                                    <div className="col-lg-6 col-md-6 col-sm-12 single-column">
                                        <div className="single-item">
                                            <h2>18+</h2>
                                            <p>Years on the market</p>
                                        </div>
                                    </div>
                                    <div className="col-lg-6 col-md-6 col-sm-12 single-column">
                                        <div className="single-item">
                                            <h2>FSC</h2>
                                            <p>Regulated by authorities</p>
                                        </div>
                                    </div>
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
