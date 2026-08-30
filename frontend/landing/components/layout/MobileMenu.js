'use client'
import Link from "next/link"

const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400"

/** Same four destinations as the desktop menu, plus the portal doors. */
export default function MobileMenu({ isSidebar, handleMobileMenu, handleSidebar }) {
    return (
        <>
            <div className="mobile-menu">
                <div className="menu-backdrop" onClick={handleMobileMenu} />
                <div className="close-btn" onClick={handleMobileMenu}><i className="fas fa-times" /></div>
                <nav className="menu-box">
                    <div className="nav-logo">
                        <Link href="/"><img src="/assets/images/logo.png" alt="Algorik" title="" /></Link>
                    </div>
                    <div className="menu-outer">
                        <ul className="navigation clearfix">
                            <li><Link href="/" onClick={handleMobileMenu}>Home</Link></li>
                            <li><Link href="/platform" onClick={handleMobileMenu}>Platform</Link></li>
                            <li><Link href="/about" onClick={handleMobileMenu}>Company</Link></li>
                            <li><Link href="/contact" onClick={handleMobileMenu}>Contact</Link></li>
                            <li><a href={`${PORTAL}/sign-in`}>Sign In</a></li>
                            <li><a href={`${PORTAL}/sign-up`}>Get Started</a></li>
                        </ul>
                    </div>
                </nav>
            </div>
        </>
    )
}
