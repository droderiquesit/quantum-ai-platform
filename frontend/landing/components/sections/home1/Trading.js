import Link from "next/link"

export default function Trading() {
  return (
    <>
      <section className="trading-section pt_100 pb_100">
            <div className="auto-container">
                <div className="sec-title centred pb_60">
                    <span className="sub-title mb_14">Coverage</span>
                    <h2>What Algorik watches</h2>
                </div>
                <div className="inner-container clearfix">
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-1.png" alt=""/></figure>
                            <h3>Equities</h3>
                            <p>Bars, trades, quotes and depth absorbed each cycle into a bitemporal world model.</p>
                            <div className="btn-box"><Link href="/platform" className="theme-btn btn-one">See how it’s modelled</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-2.png" alt=""/></figure>
                            <h3>Currencies</h3>
                            <p>Modelled and paper-traded inside per-venue and per-cell capital bounds, as sources come online.</p>
                            <div className="btn-box"><Link href="/platform" className="theme-btn btn-one">See how it’s modelled</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-3.png" alt=""/></figure>
                            <h3>Crypto & on-chain</h3>
                            <p>Chain state read at configured depth — blocks, gas, AMMs — with reorgs handled, never assumed away.</p>
                            <div className="btn-box"><Link href="/platform" className="theme-btn btn-one">See how it’s modelled</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-4.png" alt=""/></figure>
                            <h3>Commodities</h3>
                            <p>Futures curves modelled alongside the macro releases that move them.</p>
                            <div className="btn-box"><Link href="/platform" className="theme-btn btn-one">See how it’s modelled</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-5.png" alt=""/></figure>
                            <h3>Macro & rates</h3>
                            <p>Built to absorb releases at the instant they became knowable — bitemporal by design, so backtests cannot cheat.</p>
                            <div className="btn-box"><Link href="/platform" className="theme-btn btn-one">See how it’s modelled</Link></div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
