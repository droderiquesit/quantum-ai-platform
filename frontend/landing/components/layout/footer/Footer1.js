import Link from "next/link"
import { CONTACT_EMAIL, FOOTER_COLUMNS, POSTURE, SIGN_IN, SIGN_UP } from "@/lib/site"

/**
 * The footer, generated from the same IA declaration as the navigation.
 *
 * What was removed and why:
 *  - `logo-3.png` is the template vendor's own lockup — their wordmark, not
 *    ours. It shipped in the footer of every page, beside Algorik's logo.
 *  - The copyright line assigned the site's rights to that same vendor.
 *  - A QR code from the template package, encoding whoever the vendor pointed
 *    it at. Nobody here can say what it resolves to, so it cannot ship.
 *  - App Store and Play links pointing at "/". The phone app is the portal's
 *    installed PWA; there is no store listing to send anyone to.
 *  - Four social icons linking to "/". Algorik has no accounts to follow.
 */
export default function Footer1() {
    return (
        <footer className="main-footer">
            <div className="widget-section p_relative pt_70 pb_80">
                <div className="auto-container">
                    <div className="row clearfix">
                        <div className="col-lg-8 col-md-12 col-sm-12 big-column">
                            <div className="row clearfix">
                                {FOOTER_COLUMNS.map((column) => (
                                    <div key={column.title} className="col-lg-3 col-md-6 col-sm-12 footer-column">
                                        <div className="footer-widget links-widget">
                                            <div className="widget-title mb_11">
                                                <h3>{column.title}</h3>
                                            </div>
                                            <div className="widget-content">
                                                <ul className="links-list clearfix">
                                                    {column.links.map((link) => (
                                                        <li key={link.href}><Link href={link.href}>{link.label}</Link></li>
                                                    ))}
                                                </ul>
                                            </div>
                                        </div>
                                    </div>
                                ))}
                            </div>
                            <div className="footer-lower">
                                <figure className="footer-logo">
                                    <Link href="/"><img src="/assets/images/logo.png" alt="Algorik" width="140" height="34" /></Link>
                                </figure>
                            </div>
                        </div>
                        <div className="col-lg-4 col-md-6 col-sm-12 footer-column">
                            <div className="footer-widget logo-widget ml_80">
                                <div className="widget-content">
                                    <div className="widget-title mb_11"><h3>The doors</h3></div>
                                    <ul className="links-list clearfix">
                                        <li><a href={SIGN_UP}>Get Started</a></li>
                                        <li><a href={SIGN_IN}>Sign In</a></li>
                                        <li><a href={`mailto:${CONTACT_EMAIL}`}>{CONTACT_EMAIL}</a></li>
                                    </ul>
                                    <p className="footer-note">
                                        {POSTURE}, end to end. No control on this platform submits a live order,
                                        and no page here offers one.
                                    </p>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
            <div className="footer-bottom">
                <div className="auto-container">
                    <div className="bottom-inner">
                        <p>Copyright {new Date().getFullYear()} Algorik. All rights reserved.</p>
                        <p className="footer-posture">
                            <Link href="/legal/risk-disclosures">Risk disclosures</Link>
                            <span aria-hidden="true"> · </span>
                            <Link href="/legal/terms">Terms</Link>
                            <span aria-hidden="true"> · </span>
                            <Link href="/legal/privacy">Privacy</Link>
                        </p>
                    </div>
                </div>
            </div>
        </footer>
    )
}
