/**
 * The route-transition preloader.
 *
 * It spelled out the template vendor's own product name one letter per span,
 * animated, on every navigation. It also used `class` rather than `className`,
 * which React reports as an error in the console on every render.
 */
const LETTERS = ["A", "l", "g", "o", "r", "i", "k"]

export default function Preloader() {
    return (
        <div className="loader-wrap">
            <div className="preloader">
                <div id="handle-preloader" className="handle-preloader">
                    <div className="animation-preloader">
                        <div className="spinner"></div>
                        <div className="txt-loading">
                            {LETTERS.map((letter, index) => (
                                <span key={index} data-text-preloader={letter} className="letters-loading">
                                    {letter}
                                </span>
                            ))}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    )
}
