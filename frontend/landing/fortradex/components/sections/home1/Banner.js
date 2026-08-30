'use client'
import { Autoplay, Navigation, Pagination } from "swiper/modules"
import { Swiper, SwiperSlide } from "swiper/react"
import Link from "next/link"

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
                        <div className="bg-layer" style={{ backgroundImage: "url(assets/images/banner/banner-1.jpg)" }}></div>
                        <div className="content-box">
                            <h2>Trading for Anyone. Anywhere. Anytime.</h2>
                            <p>Trade over 1000 Instruments. Forex, CFDs on Stock Indices, Commodities, Stocks, Metals and Energies.</p>
                            <div className="btn-box">
                                <Link href="/" className="theme-btn btn-one">Create Account</Link>
                            </div>
                        </div>
                    </SwiperSlide>
                    <SwiperSlide className="slide-item p_relative">
                        <div className="bg-layer" style={{ backgroundImage: "url(assets/images/banner/banner-2.jpg)" }}></div>
                        <div className="content-box">
                            <h2>Trading for Anyone. Anywhere. Anytime.</h2>
                            <p>Trade over 1000 Instruments. Forex, CFDs on Stock Indices, Commodities, Stocks, Metals and Energies.</p>
                            <div className="btn-box">
                                <Link href="/" className="theme-btn btn-one">Create Account</Link>
                            </div>
                        </div>
                    </SwiperSlide>
                    <SwiperSlide className="slide-item p_relative">
                        <div className="bg-layer" style={{ backgroundImage: "url(assets/images/banner/banner-3.jpg)" }}></div>
                        <div className="content-box">
                            <h2>Trading for Anyone. Anywhere. Anytime.</h2>
                            <p>Trade over 1000 Instruments. Forex, CFDs on Stock Indices, Commodities, Stocks, Metals and Energies.</p>
                            <div className="btn-box">
                                <Link href="/" className="theme-btn btn-one">Create Account</Link>
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
