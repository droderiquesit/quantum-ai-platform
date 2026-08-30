'use client'
import { useState } from "react"
import VideoPopup from "@/components/elements/VideoPopup"
export default function About() {

    const [isActive, setIsActive] = useState({
        status: false,
        key: 1,
    })

    const handleToggle = (key) => {
        if (isActive.key === key) {
            setIsActive({
                status: false,
            })
        } else {
            setIsActive({
                status: true,
                key,
            })
        }
    }

  return (
    <>
      <section className="about-section pt_100 pb_100">
            <div className="auto-container">
                <div className="row align-items-center">
                    <div className="col-lg-6 col-md-12 col-sm-12 content-column">
                        <div className="content_block_one">
                            <div className="content-box mr_80">
                                <div className="sec-title pb_30">
                                    <span className="sub-title mb_14">The platform</span>
                                    <h2>What Algorik is</h2>
                                </div>
                                <ul className="accordion-box">
                                    <li className="accordion block active-block">
                                        <div className={isActive.key == 1 ? "acc-btn active" : "acc-btn"} onClick={() => handleToggle(1)}>
                                            <div className="icon-box"><i className="icon-29"></i></div>
                                            <h3>Who we are</h3>
                                        </div>
                                        <div className={isActive.key == 1 ? "acc-content current" : "acc-content"}>
                                            <div className="content">
                                                <p>A research desk’s decision loop: market sensing, an agent panel that argues before it acts, and execution against simulators only. Paper trading is the product, not a phase.</p>
                                            </div>
                                        </div>
                                    </li>
                                    <li className="accordion block">
                                        <div className={isActive.key == 2 ? "acc-btn active" : "acc-btn"} onClick={() => handleToggle(2)}>
                                            <div className="icon-box"><i className="icon-29"></i></div>
                                            <h3>What we do</h3>
                                        </div>
                                        <div className={isActive.key == 2 ? "acc-content current" : "acc-content"}>
                                            <div className="content">
                                                <p>Each cycle detects opportunities, sizes them under hard limits checked before an order exists, executes on paper, and scores its own reasoning afterwards.</p>
                                            </div>
                                        </div>
                                    </li>
                                    <li className="accordion block">
                                        <div className={isActive.key == 3 ? "acc-btn active" : "acc-btn"} onClick={() => handleToggle(3)}>
                                            <div className="icon-box"><i className="icon-29"></i></div>
                                            <h3>How it works</h3>
                                        </div>
                                        <div className={isActive.key == 3 ? "acc-content current" : "acc-content"}>
                                            <div className="content">
                                                <p>An eight-stage loop writes every step to a hash-chained event log. If a number is on a screen, the platform can show where it came from.</p>
                                            </div>
                                        </div>
                                    </li>
                                </ul>
                            </div>
                        </div>
                    </div>
                    <div className="col-lg-6 col-md-12 col-sm-12 video-column">
                        <div className="video_block_one">
                            <div className="video-box z_1 p_relative ml_70 centred">
                                <div className="video-inner">
                                    <div className="bg-layer" style={{ backgroundImage: "url(assets/images/resource/video-1.jpg)" }}></div>
                                    <div className="video-content">
                                        <VideoPopup />
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </section>
    </>
  )
}
