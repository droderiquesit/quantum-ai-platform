import Link from "next/link"
import Brand from "../Brand"
import Menu from "../Menu"
import MobileMenu from "../MobileMenu"
import { POSTURE, SIGN_IN, SIGN_UP } from "@/lib/site"

/**
 * The one header this site has.
 *
 * The template shipped five; four were unreachable and carried a fabricated
 * phone number, a "£20 Discount" strip and links to demo routes that 404.
 * They are gone rather than dormant — an unused component is a component
 * somebody imports later.
 *
 * The search toggler went with them: a static twelve-page site had a search
 * box whose form submitted to the home page and discarded the query.
 */
export default function Header1({ scroll, handleMobileMenu }) {
    return (
        <header className={`main-header header-style-one ${scroll ? "fixed-header" : ""}`}>
            <div className="header-top">
                <div className="large-container">
                    <div className="top-inner">
                        {/* Posture, in the words the platform's own rules require. The
                            template's headset icon went with the support line it belonged
                            to: this strip is a statement of posture, not an offer of help. */}
                        <div className="support-box">
                            <span>{POSTURE} — simulated execution only</span>
                        </div>
                        <div className="option-block">
                            <a href={SIGN_UP} className="theme-btn btn-one mr_10">Get Started</a>
                            <a href={SIGN_IN} className="theme-btn btn-two">Sign In</a>
                        </div>
                    </div>
                </div>
            </div>

            <div className="header-lower">
                <div className="large-container">
                    <div className="outer-box">
                        <figure className="logo-box">
                            <Link href="/" aria-label="Algorik home"><Brand height={38} /></Link>
                        </figure>
                        <div className="menu-area clearfix">
                            <div className="mobile-nav-toggler" onClick={handleMobileMenu}
                                role="button" tabIndex={0} aria-label="Open navigation">
                                <i className="icon-bar"></i>
                                <i className="icon-bar"></i>
                                <i className="icon-bar"></i>
                            </div>
                            <nav className="main-menu navbar-expand-md navbar-light">
                                <div className="collapse navbar-collapse show clearfix" id="navbarSupportedContent">
                                    <Menu />
                                </div>
                            </nav>
                        </div>
                    </div>
                </div>
            </div>

            <div className={`sticky-header ${scroll ? "animated slideInDown" : ""}`}>
                <div className="large-container">
                    <div className="outer-box">
                        <figure className="logo-box">
                            <Link href="/" aria-label="Algorik home"><Brand height={38} /></Link>
                        </figure>
                        <div className="menu-area clearfix">
                            <nav className="main-menu clearfix">
                                <div className="collapse navbar-collapse show clearfix">
                                    <Menu />
                                </div>
                            </nav>
                        </div>
                    </div>
                </div>
            </div>

            <MobileMenu handleMobileMenu={handleMobileMenu} />
        </header>
    )
}
