import Link from "next/link"

const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400"

export default function Footer1() {
    return (
        <>
        <footer className="main-footer">
            <div className="widget-section p_relative pt_70 pb_80">
                <div className="auto-container">
                    <div className="row clearfix">
                        <div className="col-lg-8 col-md-12 col-sm-12 big-column">
                            <div className="row clearfix">
                                <div className="col-lg-3 col-md-6 col-sm-12 footer-column">
                                    <div className="footer-widget links-widget">
                                        <div className="widget-title mb_11">
                                            <h3>Company</h3>
                                        </div>
                                        <div className="widget-content">
                                            <ul className="links-list clearfix">
                                                <li><Link href="/about">Who we are</Link></li>
                                                <li><Link href="/platform">The platform</Link></li>
                                                <li><Link href="/contact">Contact us</Link></li>
                                                <li><a href={`${PORTAL}/sign-in`}>Sign In</a></li>
                                                <li><a href={`${PORTAL}/sign-up`}>Get Started</a></li>
                                            </ul>
                                        </div>
                                    </div>
                                </div>
                                <div className="col-lg-3 col-md-6 col-sm-12 footer-column">
                                    <div className="footer-widget links-widget">
                                        <div className="widget-title mb_11">
                                            <h3>Coverage</h3>
                                        </div>
                                        <div className="widget-content">
                                            <ul className="links-list clearfix">
                                                <li><Link href="/platform">Equities</Link></li>
                                                <li><Link href="/platform">Currencies</Link></li>
                                                <li><Link href="/platform">Crypto &amp; on-chain</Link></li>
                                                <li><Link href="/platform">Commodities</Link></li>
                                                <li><Link href="/platform">Macro &amp; rates</Link></li>
                                            </ul>
                                        </div>
                                    </div>
                                </div>
                                <div className="col-lg-3 col-md-6 col-sm-12 footer-column">
                                    <div className="footer-widget links-widget">
                                        <div className="widget-title mb_11">
                                            <h3>The loop</h3>
                                        </div>
                                        <div className="widget-content">
                                            <ul className="links-list clearfix">
                                                <li><Link href="/platform">The eight-stage loop</Link></li>
                                                <li><Link href="/platform">Agent panel review</Link></li>
                                                <li><Link href="/platform">Risk refusals</Link></li>
                                                <li><Link href="/platform">Hash-chained audit log</Link></li>
                                                <li><Link href="/platform">Quantum with a baseline</Link></li>
                                            </ul>
                                        </div>
                                    </div>
                                </div>
                                <div className="col-lg-3 col-md-6 col-sm-12 footer-column">
                                    <div className="footer-widget links-widget">
                                        <div className="widget-title mb_25">
                                            <h3>Trust</h3>
                                        </div>
                                        <div className="widget-content">
                                            <ul className="links-list clearfix">
                                                <li><Link href="/about">Paper trading, always</Link></li>
                                                <li><Link href="/platform">Auditability</Link></li>
                                                <li><Link href="/platform">Security posture</Link></li>
                                                <li><Link href="/contact">Talk to the desk</Link></li>
                                            </ul>
                                        </div>
                                    </div>
                                </div>
                            </div>
                            <div className="footer-lower">
                                <figure className="footer-logo"><Link href="/"><img src="assets/images/logo.png" alt="Algorik"/></Link></figure>
                                
                            </div>
                        </div>
                        <div className="col-lg-4 col-md-6 col-sm-12 footer-column">
                            <div className="footer-widget logo-widget centred ml_80">
                                <div className="widget-content">
                                    <figure className="footer-logo mb_15"><Link href="/"><img src="assets/images/logo-3.png" alt=""/></Link></figure>
                                    <p>Paper trading, end to end — no control on this platform submits a live order.</p>
                                    <div className="scanner-box mb_30"><img src="assets/images/icons/icon-3.png" alt=""/></div>
                                    <ul className="download-list clearfix">
                                        <li><Link href="/"><i className="fab fa-apple"></i></Link></li>
                                        <li><Link href="/"><img src="assets/images/icons/icon-2.png" alt=""/></Link></li>
                                        <li><Link href="/"><i className="fab fa-android"></i></Link></li>
                                    </ul>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
            <div className="footer-bottom">
                <div className="auto-container">
                    <div className="bottom-inner">
                        <p>Copyright {new Date().getFullYear()} <Link href="/">ForTradex</Link> All Rights Reserved.</p>
                        <ul className="social-links">
                            <li><h5>Follow Us On:</h5></li>
                            <li><Link href="/"><i className="icon-12"></i></Link></li>
                            <li><Link href="/"><i className="icon-13"></i></Link></li>
                            <li><Link href="/"><i className="icon-14"></i></Link></li>
                            <li><Link href="/"><i className="icon-15"></i></Link></li>
                        </ul>
                    </div>
                </div>
            </div>
        </footer>

        </>
    )
}
