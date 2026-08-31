'use client'
import Link from "next/link"
import Brand from "./Brand"
import { NAV_FLAT, POSTURE, SIGN_IN, SIGN_UP } from "@/lib/site"

/**
 * Every destination, flat.
 *
 * A nested mobile menu hides pages behind a tap on exactly the width where
 * most visitors arrive, and a hidden legal page is the one a regulator looks
 * for first. So the phone gets the whole list, not a subset of it.
 */
export default function MobileMenu({ handleMobileMenu }) {
    return (
        <div className="mobile-menu">
            <div className="menu-backdrop" onClick={handleMobileMenu} />
            <div className="close-btn" onClick={handleMobileMenu}><i className="fas fa-times" /></div>
            <nav className="menu-box">
                <div className="nav-logo">
                    <Link href="/" onClick={handleMobileMenu}>
                        <Brand height={32} onDark />
                    </Link>
                </div>
                <div className="menu-outer">
                    <ul className="navigation clearfix">
                        {NAV_FLAT.map((item) => (
                            <li key={item.href}>
                                <Link href={item.href} onClick={handleMobileMenu}>{item.label}</Link>
                            </li>
                        ))}
                        <li><a href={SIGN_IN}>Sign In</a></li>
                        <li><a href={SIGN_UP}>Get Started</a></li>
                    </ul>
                </div>
                <p className="mobile-posture">{POSTURE} — simulated execution only</p>
            </nav>
        </div>
    )
}
