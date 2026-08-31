import Link from "next/link"

export default function Breadcrumb({ breadcrumbTitle, breadcrumbLede }) {
    return (
        <section className="page-title centred pt_90 pb_0">
            <div className="pattern-layer rotate-me" style={{ backgroundImage: 'url(/assets/images/shape/shape-34.png)' }}></div>
            <div className="auto-container">
                <div className="content-box">
                    <h1>{breadcrumbTitle}</h1>
                    {breadcrumbLede && <p className="breadcrumb-lede">{breadcrumbLede}</p>}
                    <ul className="bread-crumb clearfix">
                        <li><Link href="/">Home</Link></li>
                        <li>{breadcrumbTitle}</li>
                    </ul>
                </div>
            </div>
        </section>
    )
}
