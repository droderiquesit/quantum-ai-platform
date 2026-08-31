'use client'
import { useState } from "react"
import { LoopDiagram } from "@/components/art/Diagrams"

/**
 * What Algorik is.
 *
 * The right-hand column held a video lightbox over `video-1.jpg` — a grey
 * placeholder with no video behind it, so the column rendered as an empty grey
 * rectangle. There is no product film to show yet; the loop diagram states
 * something true instead of promising something that does not exist.
 */
const ITEMS = [
    ["Who we are", "A research desk’s decision loop: market sensing, an agent panel that argues before it acts, and execution against simulators only. Paper trading is the product, not a phase."],
    ["What we do", "Each cycle detects opportunities, sizes them under hard limits checked before an order exists, executes on paper, and scores its own reasoning afterwards."],
    ["How it works", "An eight-stage loop writes every step to a hash-chained event log. If a number is on a screen, the platform can show where it came from."],
]

export default function About() {
    const [open, setOpen] = useState(0)

    return (
        <section className="about-section pt_100 pb_100" id="what-it-is">
            <div className="auto-container">
                <div className="algorik-split">
                    <div className="content_block_one">
                        <div className="content-box">
                            <div className="sec-title pb_30">
                                <span className="sub-title mb_14">The platform</span>
                                <h2>What Algorik is</h2>
                            </div>
                            <ul className="accordion-box">
                                {ITEMS.map(([title, body], index) => (
                                    <li key={title} className={`accordion block ${open === index ? "active-block" : ""}`}>
                                        <div className={open === index ? "acc-btn active" : "acc-btn"}
                                            role="button" tabIndex={0}
                                            aria-expanded={open === index}
                                            onClick={() => setOpen(open === index ? -1 : index)}
                                            onKeyDown={(event) => {
                                                if (event.key === "Enter" || event.key === " ") {
                                                    event.preventDefault()
                                                    setOpen(open === index ? -1 : index)
                                                }
                                            }}>
                                            <div className="icon-box"><i className="icon-29"></i></div>
                                            <h3>{title}</h3>
                                        </div>
                                        <div className={open === index ? "acc-content current" : "acc-content"}>
                                            <div className="content"><p>{body}</p></div>
                                        </div>
                                    </li>
                                ))}
                            </ul>
                        </div>
                    </div>
                    <figure className="algorik-figure">
                        <LoopDiagram />
                        <figcaption>
                            One cycle, eight stages. Every stage reports what it produced and what it refused.
                        </figcaption>
                    </figure>
                </div>
            </div>
        </section>
    )
}
