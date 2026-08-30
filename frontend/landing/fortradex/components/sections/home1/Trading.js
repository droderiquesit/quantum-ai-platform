import Link from "next/link"

export default function Trading() {
  return (
    <>
      <section className="trading-section pt_100 pb_100">
            <div className="auto-container">
                <div className="sec-title centred pb_60">
                    <span className="sub-title mb_14">Trading Platforms</span>
                    <h2>Things We Trade</h2>
                </div>
                <div className="inner-container clearfix">
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-1.png" alt=""/></figure>
                            <h3><a href="markets-details.html">Crypto Trading</a></h3>
                            <p>One of the primary methods of gold trading is through the spot</p>
                            <div className="btn-box"><Link href="/markets-details" className="theme-btn btn-one">Start Trading Now</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-2.png" alt=""/></figure>
                            <h3><a href="markets-details.html">Shares Trading</a></h3>
                            <p>One of the primary methods of gold trading is through the spot</p>
                            <div className="btn-box"><Link href="/markets-details" className="theme-btn btn-one">Start Trading Now</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-3.png" alt=""/></figure>
                            <h3><a href="markets-details.html">Gold Trading</a></h3>
                            <p>One of the primary methods of gold trading is through the spot</p>
                            <div className="btn-box"><Link href="/markets-details" className="theme-btn btn-one">Start Trading Now</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-4.png" alt=""/></figure>
                            <h3><a href="markets-details.html">Currency Trading</a></h3>
                            <p>One of the primary methods of gold trading is through the spot</p>
                            <div className="btn-box"><Link href="/markets-details" className="theme-btn btn-one">Start Trading Now</Link></div>
                        </div>
                    </div>
                    <div className="trading-block-one">
                        <div className="inner-box">
                            <figure className="image-box"><img src="assets/images/resource/trading-5.png" alt=""/></figure>
                            <h3><a href="markets-details.html">Silver Trading</a></h3>
                            <p>One of the primary methods of gold trading is through the spot</p>
                            <div className="btn-box"><Link href="/markets-details" className="theme-btn btn-one">Start Trading Now</Link></div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
