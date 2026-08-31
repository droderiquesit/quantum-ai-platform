'use client'
import { Autoplay, Navigation, Pagination } from "swiper/modules"
import { Swiper, SwiperSlide } from "swiper/react"
import { HeroBaseline, HeroBoundary, HeroLoop } from "@/components/art/Hero"
import { SIGN_UP } from "@/lib/site"

const swiperOptions = {
    modules: [Autoplay, Pagination, Navigation],
    slidesPerView: 1,
    spaceBetween: 30,
    autoplay: { delay: 10000, disableOnInteraction: false },
    loop: true,
    navigation: { nextEl: '.owl-prev', prevEl: '.owl-next' },
    pagination: { el: '.swiper-pagination', clickable: true },
}

/**
 * Three slides, each with a drawing of what it claims. The template filled the
 * right half with a grey placeholder rectangle; without it the headline sat in
 * an empty box on any screen wider than a laptop.
 */
const SLIDES = [
    {
        key: "posture",
        gradient: "linear-gradient(120deg, #050709 30%, #0b2b22 75%, rgba(16,185,129,0.35) 130%)",
        title: "Institutional intelligence. Paper-trading discipline.",
        body: "Algorik senses global markets, reasons with a panel of AI agents, and executes only against simulators. Every decision is reproducible from its own audit log.",
        Art: HeroBoundary,
    },
    {
        key: "loop",
        gradient: "linear-gradient(120deg, #050709 30%, #101433 75%, rgba(99,102,241,0.35) 130%)",
        title: "An eight-stage loop you can audit.",
        body: "Sense, understand, discover, reason, simulate, decide, act, learn — each stage writes to a hash-chained record, so “why” always has an answer.",
        Art: HeroLoop,
    },
    {
        key: "quantum",
        gradient: "linear-gradient(120deg, #050709 30%, #0b2330 75%, rgba(14,165,233,0.35) 130%)",
        title: "Quantum research, honestly measured.",
        body: "Quantum optimisation runs only where it beats the classical baseline computed on every decision. Research with a control group, not a slogan.",
        Art: HeroBaseline,
    },
]

export default function Banner() {
    return (
        <section className="banner-section p_relative pt_20">
            <div className="large-container">
                <Swiper {...swiperOptions} className="theme_carousel owl-theme banner-carousel">
                    {SLIDES.map(({ key, gradient, title, body, Art }) => (
                        <SwiperSlide key={key} className="slide-item p_relative">
                            <div className="bg-layer" style={{ backgroundImage: gradient }}></div>
                            <div className="content-box">
                                <h2>{title}</h2>
                                <p>{body}</p>
                                <div className="btn-box">
                                    <a href={SIGN_UP} className="theme-btn btn-one">Get Started</a>
                                </div>
                            </div>
                            <div className="banner-art" aria-hidden="false"><Art /></div>
                        </SwiperSlide>
                    ))}
                    <div className="owl-dots">
                        <div className="swiper-pagination"></div>
                    </div>
                </Swiper>
            </div>
        </section>
    )
}
