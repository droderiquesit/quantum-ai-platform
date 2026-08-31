import Link from "next/link"
import { ChainMark, CommoditiesMark, CurrenciesMark, EquitiesMark, MacroMark } from "@/components/art/Marks"

/**
 * Coverage. The five illustrations replace `trading-1..5.png`, which were grey
 * rectangles with "206x211" printed on them — the template's own placeholders,
 * shipped to production and rendering as grey boxes on the home page.
 */
const COVERAGE = [
    [EquitiesMark, "Equities", "Bars, trades, quotes and depth absorbed each cycle into a bitemporal world model."],
    [CurrenciesMark, "Currencies", "Modelled and paper-traded inside per-venue and per-cell capital bounds, as sources come online."],
    [ChainMark, "Crypto & on-chain", "Chain state read at configured depth — blocks, gas, AMMs — with reorgs handled, never assumed away."],
    [CommoditiesMark, "Commodities", "Futures curves modelled alongside the macro releases that move them."],
    [MacroMark, "Macro & rates", "Absorbed at the instant each release became knowable — bitemporal by design, so backtests cannot cheat."],
]

export default function Trading() {
    return (
        <section className="trading-section pt_100 pb_100" id="coverage">
            <div className="auto-container">
                <div className="sec-title centred pb_60">
                    <span className="sub-title mb_14">Coverage</span>
                    <h2>What Algorik watches</h2>
                </div>
                <div className="inner-container clearfix">
                    {COVERAGE.map(([Mark, title, body]) => (
                        <div key={title} className="trading-block-one">
                            <div className="inner-box">
                                <span className="algorik-mark"><Mark /></span>
                                <h3>{title}</h3>
                                <p>{body}</p>
                                <div className="btn-box">
                                    <Link href="/platform" className="theme-btn btn-one">See how it&rsquo;s modelled</Link>
                                </div>
                            </div>
                        </div>
                    ))}
                </div>
            </div>
        </section>
    )
}
