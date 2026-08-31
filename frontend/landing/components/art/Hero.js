/**
 * The three hero marks.
 *
 * The template's banner filled the right half of the slide with
 * `banner-img-*.png` — grey placeholder rectangles. On a 1440px screen the
 * slide read as a headline floating in an empty box. These are drawn for a
 * dark background and say the same thing the slide's headline says.
 */

const GREEN = "#10b981"
const FAINT = "rgba(255,255,255,0.20)"
const DIM = "rgba(255,255,255,0.55)"

function HeroCanvas({ children, label }) {
    return (
        <svg viewBox="0 0 420 340" role="img" aria-label={label}
            style={{ width: "100%", height: "auto", display: "block" }}>
            {children}
        </svg>
    )
}

/** Slide one — the three layers, as a stack nothing gets through. */
export function HeroBoundary() {
    return (
        <HeroCanvas label="Three layers refusing a live order">
            <rect x="120" y="14" width="180" height="42" rx="10" fill="none" stroke={DIM} strokeWidth="2" />
            <text x="210" y="41" textAnchor="middle" fontSize="14" fill="#ffffff">A live-order request</text>
            <path d="M210 56 v22" stroke={DIM} strokeWidth="2" />
            {[0, 1, 2].map((i) => {
                const y = 84 + i * 74
                return (
                    <g key={i}>
                        <rect x="40" y={y} width="340" height="56" rx="12"
                            fill="rgba(255,255,255,0.05)" stroke={FAINT} strokeWidth="2" />
                        <rect x="40" y={y} width="6" height="56" rx="3" fill={GREEN} />
                        <text x="66" y={y + 34} fontSize="15" fontWeight="600" fill="#ffffff">
                            {["Infrastructure refuses", "Start-up refuses", "The types refuse"][i]}
                        </text>
                        <g transform={`translate(348 ${y + 28})`}>
                            <circle r="14" fill={GREEN} />
                            <path d="M-6 -6 L6 6 M6 -6 L-6 6" stroke="#04120c" strokeWidth="3" strokeLinecap="round" />
                        </g>
                        {i < 2 && <path d={`M210 ${y + 56} v18`} stroke={FAINT} strokeWidth="2" />}
                    </g>
                )
            })}
        </HeroCanvas>
    )
}

/** Slide two — the eight stages, as a ring on dark. */
export function HeroLoop() {
    const stages = ["Sense", "Understand", "Discover", "Reason", "Simulate", "Decide", "Act", "Learn"]
    const cx = 210
    const cy = 168
    const r = 108
    return (
        <HeroCanvas label="An eight-stage loop, every step recorded">
            <circle cx={cx} cy={cy} r={r} fill="none" stroke={FAINT} strokeWidth="2" />
            <circle cx={cx} cy={cy} r={r} fill="none" stroke={GREEN} strokeWidth="3"
                strokeDasharray="380 300" strokeLinecap="round" transform={`rotate(-96 ${cx} ${cy})`} />
            <text x={cx} y={cy - 4} textAnchor="middle" fontSize="14" fontWeight="700" fill="#ffffff">ONE CYCLE</text>
            <text x={cx} y={cy + 16} textAnchor="middle" fontSize="11" fill={DIM}>hash-chained</text>
            {stages.map((name, i) => {
                const a = (i / stages.length) * Math.PI * 2 - Math.PI / 2
                const x = cx + r * Math.cos(a)
                const y = cy + r * Math.sin(a)
                const lx = cx + (r + 36) * Math.cos(a)
                const ly = cy + (r + 36) * Math.sin(a)
                const anchor = Math.abs(Math.cos(a)) < 0.25 ? "middle" : (Math.cos(a) > 0 ? "start" : "end")
                return (
                    <g key={name}>
                        <circle cx={x} cy={y} r="13" fill={i === 6 ? GREEN : "#0a1512"} stroke={i === 6 ? GREEN : DIM} strokeWidth="2" />
                        <text x={x} y={y + 4} textAnchor="middle" fontSize="11" fontWeight="700"
                            fill={i === 6 ? "#04120c" : "#ffffff"}>{i + 1}</text>
                        <text x={lx} y={ly + 4} textAnchor={anchor} fontSize="12" fill={DIM}>{name}</text>
                    </g>
                )
            })}
        </HeroCanvas>
    )
}

/** Slide three — the quantum run beside its control group. */
export function HeroBaseline() {
    return (
        <HeroCanvas label="Every quantum run paired with a classical baseline">
            <rect x="140" y="18" width="140" height="40" rx="10" fill="none" stroke={DIM} strokeWidth="2" />
            <text x="210" y="43" textAnchor="middle" fontSize="14" fill="#ffffff">One problem</text>
            <path d="M210 58 v20 M90 78 h240 M90 78 v22 M330 78 v22" fill="none" stroke={FAINT} strokeWidth="2" />
            <rect x="20" y="100" width="150" height="92" rx="12" fill="rgba(255,255,255,0.05)" stroke={FAINT} strokeWidth="2" />
            <rect x="20" y="100" width="6" height="92" rx="3" fill={DIM} />
            <text x="42" y="130" fontSize="14" fontWeight="700" fill="#ffffff">Classical</text>
            <text x="42" y="150" fontSize="12" fill={DIM}>baseline, computed</text>
            <text x="42" y="168" fontSize="12" fill={DIM}>every single time</text>
            <rect x="250" y="100" width="150" height="92" rx="12" fill="rgba(16,185,129,0.10)" stroke={GREEN} strokeWidth="2" />
            <rect x="250" y="100" width="6" height="92" rx="3" fill={GREEN} />
            <text x="272" y="130" fontSize="14" fontWeight="700" fill="#ffffff">Quantum</text>
            <text x="272" y="150" fontSize="12" fill={DIM}>kept only where it</text>
            <text x="272" y="168" fontSize="12" fill={DIM}>measures better</text>
            <path d="M90 192 v24 h240 v-24" fill="none" stroke={FAINT} strokeWidth="2" />
            <text x="210" y="248" textAnchor="middle" fontSize="14" fontWeight="700" fill="#ffffff">Compared, then kept</text>
            <text x="210" y="272" textAnchor="middle" fontSize="12" fill={DIM}>or removed — measured, not asserted</text>
        </HeroCanvas>
    )
}
