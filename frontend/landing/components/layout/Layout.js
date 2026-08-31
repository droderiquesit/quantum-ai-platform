'use client'
import { useEffect, useState } from "react"
import BackToTop from '../elements/BackToTop'
import Breadcrumb from './Breadcrumb'
import Footer1 from './footer/Footer1'
import Header1 from "./header/Header1"

/**
 * One header, one footer.
 *
 * The template's switch selected between five headers and two footers by
 * number; every page passed 1, so the other six rendered nowhere while still
 * carrying the vendor's placeholder phone number, payment-card icons and
 * links to demo routes. The switch is gone with them.
 */
export default function Layout({ breadcrumbTitle, breadcrumbLede, children }) {
    const [scroll, setScroll] = useState(false)
    const [isMobileMenu, setMobileMenu] = useState(false)

    const handleMobileMenu = () => {
        setMobileMenu((open) => {
            document.body.classList.toggle("mobile-menu-visible", !open)
            return !open
        })
    }

    useEffect(() => {
        const onScroll = () => setScroll(window.scrollY > 100)
        onScroll()
        document.addEventListener("scroll", onScroll, { passive: true })
        return () => document.removeEventListener("scroll", onScroll)
    }, [])

    return (
        <>
            <div className="page-wrapper" id="top">
                <Header1 scroll={scroll} isMobileMenu={isMobileMenu} handleMobileMenu={handleMobileMenu} />
                {breadcrumbTitle && <Breadcrumb breadcrumbTitle={breadcrumbTitle} breadcrumbLede={breadcrumbLede} />}
                <main id="main-content">{children}</main>
                <Footer1 />
            </div>
            <BackToTop scroll={scroll} />
        </>
    )
}
