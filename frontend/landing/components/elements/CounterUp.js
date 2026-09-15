'use client'
import { useEffect, useState } from 'react'
import Counter from './Counter'

export default function CounterUp({ end }) {
    const [inViewport, setInViewport] = useState(false)

    const handleScroll = () => {
        const elements = document.getElementsByClassName('count-text')
        if (elements.length > 0) {
            const element = elements[0]
            const rect = element.getBoundingClientRect()
            const isInViewport = rect.top >= 0 && rect.bottom <= window.innerHeight
            if (isInViewport && !inViewport) {
                setInViewport(true)
            }
        }
    }

    useEffect(() => {
        window.addEventListener('scroll', handleScroll)
        return () => {
            window.removeEventListener('scroll', handleScroll)
        }
    }, [])
    // The figure is rendered, and the animation is an enhancement on top of it.
    //
    // This was `{inViewport && <Counter …/>}`, so the three headline statistics
    // on the home page were an empty box until a scroll event fired — and
    // `handleScroll` measures the *first* `.count-text` on the page, which on
    // the pages that also carry a numbered list is not one of these. A figure
    // that never renders is not a statement a reader can check, and the §40.6
    // annotation wrapped around it was consequently vacuous: the claims sweep
    // found an annotation covering no quantity, which is how this was noticed
    // rather than by anyone looking at the page.
    return (
        <>
            <span className="count-text">{inViewport ? <Counter end={end} duration={20} /> : end}</span>
        </>
    )
}
