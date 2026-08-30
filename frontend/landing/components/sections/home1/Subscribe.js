

export default function Subscribe() {
  return (
    <>
      <section className="subscribe-section">
            <div className="bg-color"></div>
            <div className="auto-container">
                <div className="inner-container">
                    <div className="shape" style={{ backgroundImage: "url(assets/images/shape/shape-5.png)" }}></div>
                    <div className="row align-items-center">
                        <div className="col-lg-6 col-md-12 col-sm-12 text-column">
                            <div className="text-box">
                                <h2>Questions? Talk to the desk.</h2>
                            </div>
                        </div>
                        <div className="col-lg-6 col-md-12 col-sm-12 form-column">
                            <div className="form-inner">
                                <div className="form-group" style={{ textAlign: "right" }}>
                                        <a href="/contact" className="theme-btn btn-one">Contact us<i className="icon-26"></i></a>
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
