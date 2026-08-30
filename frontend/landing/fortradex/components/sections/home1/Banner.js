'use client'
import { Autoplay, Navigation, Pagination } from "swiper/modules"
import { Swiper, SwiperSlide } from "swiper/react"
import Link from "next/link"

const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400"

const swiperOptions = {
    modules: [Autoplay, Pagination, Navigation],
    slidesPerView: 1,
    spaceBetween: 30,
    autoplay: {
        delay: 10000,
        disableOnInteraction: false,
    },
    loop: true,

    // Navigation
    navigation: {
        nextEl: '.owl-prev',
        prevEl: '.owl-next',
    },

    // Pagination
    pagination: {
        el: '.swiper-pagination',
        clickable: true,
    },

    breakpoints: {
        320: {
            slidesPerView: 1,
            spaceBetween: 30,
        },
        575: {
            slidesPerView: 1,
            spaceBetween: 30,
        },
        767: {
            slidesPerView: 1,
            spaceBetween: 30,
        },
        991: {
            slidesPerView: 1,
            spaceBetween: 30,
        },
        1199: {
            slidesPerView: 1,
            spaceBetween: 30,
        },
        1350: {
            slidesPerView: 1,
            spaceBetween: 30,
        },
    }
}


export default function Banner() {
    return (
        <> 


        <section className="banner-section p_relative pt_20">
            <div className="large-container">
                <Swiper {...swiperOptions} className="theme_carousel owl-theme banner-carousel">
                    <SwiperSlide className="slide-item p_relative">
                        <div className="bg-layer" style={{ backgroundImage: "linear-gradient(120deg, #050709 30%, #0b2b22 75%, rgba(16,185,129,0.35) 130%)" }}></div>
                        <div className="content-box">
                            <h2>Institutional intelligence. Paper-trading discipline.</h2>
                            <p>Algorik senses global markets, reasons with a panel of AI agents, and executes only against simulators. Every decision is reproducible from its own audit log.</p>
                            <div className="btn-box">
                                <a href={`${PORTAL}/sign-up`} className="theme-btn btn-one">Get Started</a>
                            </div>
                        </div>
                    </SwiperSlide>
                    <SwiperSlide className="slide-item p_relative">
                        <div className="bg-layer" style={{ backgroundImage: "linear-gradient(120deg, #050709 30%, #101433 75%, rgba(99,102,241,0.35) 130%)" }}></div>
                        <div className="content-box">
                            <h2>An eight-stage loop you can audit.</h2>
                            <p>Sense, understand, discover, reason, simulate, decide, act, learn — each stage writes to a hash-chained record, so “why” always has an answer.</p>
                            <div className="btn-box">
                                <a href={`${PORTAL}/sign-up`} className="theme-btn btn-one">Get Started</a>
                            </div>
                        </div>
                    </SwiperSlide>
                    <SwiperSlide className="slide-item p_relative">
                        <div className="bg-layer" style={{ backgroundImage: "linear-gradient(120deg, #050709 30%, #0b2330 75%, rgba(14,165,233,0.35) 130%)" }}></div>
                        <div className="content-box">
                            <h2>Quantum research, honestly measured.</h2>
                            <p>Quantum optimisation runs only where it beats the classical baseline computed on every decision. Research with a control group, not a slogan.</p>
                            <div className="btn-box">
                                <a href={`${PORTAL}/sign-up`} className="theme-btn btn-one">Get Started</a>
                            </div>
                        </div>
                    </SwiperSlide>

                    <div className="owl-dots">
                        <div className="swiper-pagination"></div>
                    </div>
                </Swiper>
            </div>
        </section>

        </>
    )
}
