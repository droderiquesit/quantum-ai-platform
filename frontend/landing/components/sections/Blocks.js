import Link from "next/link"
import { POSTURE, SIGN_UP } from "@/lib/site"

/**
 * The page vocabulary shared by every inner page.
 *
 * Each block reuses the licensed template's own class names where one fits,
 * and adds an `algorik-*` class where the template had no equivalent — so the
 * additions are visible in a diff rather than mixed into the vendor's
 * stylesheet, which is reference material (ADR 0015).
 */

export function SectionTitle({ eyebrow, title, lede, centred = true }) {
    return (
        <div className={`sec-title ${centred ? "centred" : ""} pb_50`}>
            {eyebrow && <span className="sub-title mb_14">{eyebrow}</span>}
            <h2>{title}</h2>
            {lede && <p className="algorik-lede">{lede}</p>}
        </div>
    )
}

export function Card({ title, meta, children }) {
    return (
        <div className="algorik-card">
            <div className="algorik-card-head">
                <h3>{title}</h3>
                {meta && <span className="algorik-meta">{meta}</span>}
            </div>
            <div className="algorik-card-body">{children}</div>
        </div>
    )
}

export function CardGrid({ columns = 3, children }) {
    return <div className={`algorik-grid algorik-grid-${columns}`}>{children}</div>
}

export function Bullets({ items }) {
    return (
        <ul className="algorik-bullets">
            {items.map((item, index) => (
                <li key={index}>{item}</li>
            ))}
        </ul>
    )
}

export function NumberedList({ items }) {
    return (
        <div className="row clearfix">
            {items.map(([title, body], index) => (
                <div key={title} className="col-lg-6 col-md-6 col-sm-12 content-column">
                    <div className="process-block-one">
                        <div className="inner-box">
                            <span className="count-text">{index + 1}</span>
                            <h3>{title}</h3>
                            <p>{body}</p>
                        </div>
                    </div>
                </div>
            ))}
        </div>
    )
}

/** A diagram with its caption. The caption says what the drawing asserts. */
export function Figure({ caption, children }) {
    return (
        <figure className="algorik-figure">
            {children}
            {caption && <figcaption>{caption}</figcaption>}
        </figure>
    )
}

/**
 * The end-of-page call to action. The only outbound door on this site is the
 * portal's sign-up; there is deliberately no control here that could place an
 * order, and none that implies one exists.
 */
export function CtaBand({ title, lede, secondaryHref, secondaryLabel }) {
    return (
        <section className="subscribe-section">
            <div className="bg-color"></div>
            <div className="auto-container">
                <div className="inner-container">
                    <div className="shape" style={{ backgroundImage: "url(/assets/images/shape/shape-5.png)" }}></div>
                    <div className="row align-items-center">
                        <div className="col-lg-7 col-md-12 col-sm-12 text-column">
                            <div className="text-box">
                                <h2>{title}</h2>
                                {lede && <p className="algorik-cta-lede">{lede}</p>}
                            </div>
                        </div>
                        <div className="col-lg-5 col-md-12 col-sm-12 form-column">
                            <div className="form-inner algorik-cta-actions">
                                <a href={SIGN_UP} className="theme-btn btn-one">Get Started</a>
                                {secondaryHref && (
                                    <Link href={secondaryHref} className="theme-btn btn-two">{secondaryLabel}</Link>
                                )}
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    )
}

/** The posture strip. Wherever posture is shown, it is shown in these words. */
export function PostureNote({ children }) {
    return (
        <div className="algorik-posture">
            <span className="algorik-posture-chip">{POSTURE}</span>
            <p>{children}</p>
        </div>
    )
}

/**
 * A legal document's frame. The heading numbering lives in the page so the
 * document reads as a document, and the "draft" banner is not decoration:
 * these texts have not been reviewed by counsel and saying otherwise would be
 * the most expensive sentence on the site.
 */
export function LegalShell({ title, children }) {
    return (
        <section className="algorik-legal pt_90 pb_100">
            <div className="auto-container">
                <div className="algorik-legal-inner">
                    <div className="algorik-draft" role="note">
                        <strong>Draft.</strong> This document has not been reviewed by counsel, carries no
                        effective date, and is not in force. It is published so the intent is
                        inspectable — not so it can be relied upon.
                    </div>
                    <h2>{title}</h2>
                    {children}
                </div>
            </div>
        </section>
    )
}

export function LegalSection({ heading, children }) {
    return (
        <section className="algorik-legal-section">
            <h3>{heading}</h3>
            {children}
        </section>
    )
}
