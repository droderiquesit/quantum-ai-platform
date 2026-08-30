import Layout from "@/components/layout/Layout"

/**
 * Contact, without invention. This deployment has no public office, phone
 * or inbox to print, and a fabricated address on a contact page is the
 * fastest possible way to teach visitors the rest of the site lies too.
 * The cards state what is true; the channels arrive with deployment
 * configuration, not with copy.
 */
export default function Contact() {
    return (
        <div className="boxed_wrapper">
            <Layout headerStyle={1} footerStyle={1} breadcrumbTitle="Contact">
                <section className="contact-section pt_90 pb_100">
                    <div className="auto-container">
                        <div className="info-inner pb_25">
                            <div className="row clearfix">
                                <div className="col-lg-4 col-md-6 col-sm-12 info-column">
                                    <div className="single-info">
                                        <div className="icon-box"><i className="icon-45"></i></div>
                                        <h4>The desk</h4>
                                        <p>Algorik is operated as a research and risk desk. Every question about a decision has an answer in the audit log.</p>
                                    </div>
                                </div>
                                <div className="col-lg-4 col-md-6 col-sm-12 info-column">
                                    <div className="single-info">
                                        <div className="icon-box"><i className="icon-45"></i></div>
                                        <h4>Posture</h4>
                                        <p>Paper trading, end to end. Simulated execution only — no control on this platform submits a live order.</p>
                                    </div>
                                </div>
                                <div className="col-lg-4 col-md-6 col-sm-12 info-column">
                                    <div className="single-info">
                                        <div className="icon-box"><i className="icon-45"></i></div>
                                        <h4>Channels</h4>
                                        <p>Support and sales channels are wired at deployment. Nothing is printed here that does not answer.</p>
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>
                </section>
            </Layout>
        </div>
    )
}
