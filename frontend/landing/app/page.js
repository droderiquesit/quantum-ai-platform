import Layout from "@/components/layout/Layout"
import Banner from "@/components/sections/home1/Banner"
import About from "@/components/sections/home1/About"
import Funfact from "@/components/sections/home1/Funfact"
import Trading from "@/components/sections/home1/Trading"
import Process from "@/components/sections/home1/Process"
import Apps from "@/components/sections/home1/Apps"
import Subscribe from "@/components/sections/home1/Subscribe"
import { BoundaryDiagram } from "@/components/art/Diagrams"
import { Figure, SectionTitle } from "@/components/sections/Blocks"

export default function Home() {
    return (
        <div className="boxed_wrapper">
            <Layout>
                <Banner />
                <About />
                <Funfact />
                <Trading />

                {/* The front door states the boundary rather than implying it. A
                    visitor who reads only one section should read this one. */}
                <section className="about-section pt_0 pb_100" id="paper-trading">
                    <div className="auto-container">
                        <SectionTitle
                            eyebrow="The boundary"
                            title="Paper-trading discipline"
                            lede="Algorik never submits a live order. Three independent layers hold that line, and each one catches a different way the mistake could arrive."
                        />
                        <Figure caption="Terraform catches the reviewed, committed mistake. The composition roots catch the unreviewed edit. Neither is redundant.">
                            <BoundaryDiagram />
                        </Figure>
                    </div>
                </section>

                <Process />
                <Apps />
                <Subscribe />
            </Layout>
        </div>
    )
}
