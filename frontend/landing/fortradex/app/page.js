import Layout from "@/components/layout/Layout"
import Banner from "@/components/sections/home1/Banner"
import About from "@/components/sections/home1/About"
import Funfact from "@/components/sections/home1/Funfact"
import Trading from "@/components/sections/home1/Trading"
import Process from "@/components/sections/home1/Process"
import Apps from "@/components/sections/home1/Apps"
import Subscribe from "@/components/sections/home1/Subscribe"
export default function Home() {

    return (
        <div className="boxed_wrapper">
            <Layout headerStyle={1} footerStyle={1}>
                <Banner />
                <About />
                <Funfact />
                <Trading />
                <Process />
                <Apps />
                <Subscribe />
            </Layout>
        </div>
    )
}