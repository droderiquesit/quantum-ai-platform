import Link from "next/link"

export default function Menu() {

    return (
        <>

            <ul className="navigation clearfix">
                <li className="dropdown"><Link href="/">Home</Link>
                    <ul>
                        <li><Link href="/">Home Page One</Link></li>
                        <li><Link href="/index-2">Home Page Two</Link></li>
                        <li><Link href="/index-3">Home Page Three</Link></li>
                        <li><Link href="/index-4">Home Page Four</Link></li>
                        <li><Link href="/index-5">Home Page Five</Link></li>
                    </ul>
                </li>
                <li className="dropdown"><Link href="#">Trading</Link>
                    <ul>
                        <li><Link href="/platform">Platform</Link></li>
                        <li><Link href="/account">Account</Link></li>
                        <li><Link href="/account-details">Account Deatils</Link></li>
                    </ul>
                </li>
                <li className="dropdown"><Link href="#">Market</Link>
                    <ul>
                        <li><Link href="/markets">Markets Place</Link></li>
                        <li><Link href="/markets-details">Markets Details</Link></li>
                    </ul>
                </li>
                <li className="dropdown"><Link href="#">Pages</Link>
                    <ul>
                        <li className="dropdown"><Link href="#">Education</Link>
                            <ul>
                                <li><Link href="/education">Education</Link></li>
                                <li><Link href="/education-details">Book Details</Link></li>
                            </ul>
                        </li>
                        <li className="dropdown"><Link href="#">Team</Link>
                            <ul>
                                <li><Link href="/team">Our Expert Team</Link></li>
                                <li><Link href="/team-details">Team Deatils</Link></li>
                            </ul>
                        </li>
                        <li><Link href="/about">About Us</Link></li>
                        <li><Link href="/faq">FAQ's</Link></li>
                        <li><Link href="/error">404</Link></li>
                    </ul>
                </li>
                <li className="dropdown"><Link href="#">Blog</Link>
                    <ul>
                        <li><Link href="/blog">Blog Grid</Link></li>
                        <li><Link href="/blog-2">Blog Standard</Link></li>
                        <li><Link href="/blog-details">Blog Details</Link></li>
                    </ul>
                </li> 
                <li><Link href="/contact">Contact</Link></li>
            </ul>
        </>
    )
}
