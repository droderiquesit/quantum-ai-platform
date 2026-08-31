/**
 * Diagrams that state the platform's structure.
 *
 * Every image in the template's `resource/` directory is a grey box with its
 * own pixel size written on it. Shipping those is worse than shipping nothing,
 * so the illustrated claims on this site are drawn here — as SVG, in the
 * repository, with no dependency and nothing a reader could mistake for
 * market data.
 */

const GREEN = "#10b981"
const INK = "#131615"
const LINE = "#d8e6e0"
const PAPER = "#f4f8f6"

function Canvas({ children, label, w = 640, h = 420 }) {
    return (
        <svg viewBox={`0 0 ${w} ${h}`} width={w} height={h} role="img" aria-label={label}
            style={{ width: "100%", height: "auto", display: "block" }}>
            {children}
        </svg>
    )
}

/** The eight stages, as a ring — the shape of a loop, which is the claim. */
export function LoopDiagram() {
    const stages = ["Sense", "Understand", "Discover", "Reason", "Simulate", "Decide", "Act", "Learn"]
    const cx = 320
    const cy = 210
    const r = 138
    return (
        <Canvas label="The eight-stage decision loop: sense, understand, discover, reason, simulate, decide, act, learn">
            <rect x="0" y="0" width="640" height="420" rx="18" fill={PAPER} />
            <circle cx={cx} cy={cy} r={r} fill="none" stroke={LINE} strokeWidth="2" />
            <circle cx={cx} cy={cy} r={r} fill="none" stroke={GREEN} strokeWidth="3"
                strokeDasharray="480 400" strokeLinecap="round" transform={`rotate(-96 ${cx} ${cy})`} />
            <text x={cx} y={cy - 8} textAnchor="middle" fontSize="15" fontWeight="700" fill={INK}>ONE CYCLE</text>
            <text x={cx} y={cy + 14} textAnchor="middle" fontSize="12" fill="#5d6b65">every step recorded</text>
            {stages.map((name, i) => {
                const a = (i / stages.length) * Math.PI * 2 - Math.PI / 2
                const x = cx + r * Math.cos(a)
                const y = cy + r * Math.sin(a)
                const labelX = cx + (r + 46) * Math.cos(a)
                const labelY = cy + (r + 46) * Math.sin(a)
                const anchor = Math.abs(Math.cos(a)) < 0.25 ? "middle" : (Math.cos(a) > 0 ? "start" : "end")
                return (
                    <g key={name}>
                        <circle cx={x} cy={y} r="17" fill={i === 6 ? GREEN : "#ffffff"} stroke={i === 6 ? GREEN : INK} strokeWidth="2.5" />
                        <text x={x} y={y + 5} textAnchor="middle" fontSize="13" fontWeight="700"
                            fill={i === 6 ? "#ffffff" : INK}>{i + 1}</text>
                        <text x={labelX} y={labelY + 4} textAnchor={anchor} fontSize="14" fontWeight="600" fill={INK}>{name}</text>
                    </g>
                )
            })}
        </Canvas>
    )
}

/** The three layers that refuse a live order, drawn as what stops where. */
export function BoundaryDiagram() {
    const layers = [
        ["Infrastructure as code", "refuses a live ceiling at plan time"],
        ["Composition roots", "refuse it again at process start-up"],
        ["The type system", "has no constructor that could accept one"],
    ]
    return (
        <Canvas label="Three independent layers refusing a live order: infrastructure, start-up checks, and the type system" h={360}>
            <rect x="0" y="0" width="640" height="360" rx="18" fill={PAPER} />
            <g>
                <rect x="28" y="24" width="188" height="46" rx="10" fill="#ffffff" stroke={INK} strokeWidth="2" />
                <text x="122" y="53" textAnchor="middle" fontSize="14" fontWeight="600" fill={INK}>A live-order request</text>
            </g>
            {layers.map(([title, sub], i) => {
                const y = 96 + i * 84
                return (
                    <g key={title}>
                        <line x1="122" y1={y - 26} x2="122" y2={y} stroke={INK} strokeWidth="2" />
                        <rect x="28" y={y} width="584" height="62" rx="12" fill="#ffffff" stroke={LINE} strokeWidth="2" />
                        <rect x="28" y={y} width="6" height="62" rx="3" fill={GREEN} />
                        <text x="52" y={y + 26} fontSize="15" fontWeight="700" fill={INK}>{i + 1}. {title}</text>
                        <text x="52" y={y + 46} fontSize="13" fill="#5d6b65">{sub}</text>
                        <g transform={`translate(556 ${y + 31})`}>
                            <circle r="17" fill={GREEN} />
                            <path d="M-7 -7 L7 7 M7 -7 L-7 7" stroke="#ffffff" strokeWidth="3" strokeLinecap="round" />
                        </g>
                    </g>
                )
            })}
            <text x="320" y="344" textAnchor="middle" fontSize="13" fill="#5d6b65">
                Each layer catches a mistake the others never see.
            </text>
        </Canvas>
    )
}

/** The hash chain: each record carries the digest of the one before it. */
export function ChainDiagram() {
    const blocks = [0, 1, 2, 3]
    return (
        <Canvas label="A hash-chained event log: each record carries the digest of the record before it" h={230}>
            <rect x="0" y="0" width="640" height="230" rx="18" fill={PAPER} />
            {blocks.map((i) => {
                const x = 28 + i * 152
                return (
                    <g key={i}>
                        <rect x={x} y="52" width="120" height="112" rx="12" fill="#ffffff" stroke={INK} strokeWidth="2" />
                        <rect x={x} y="52" width="120" height="30" rx="12" fill={INK} />
                        <rect x={x} y="70" width="120" height="12" fill={INK} />
                        <text x={x + 60} y="72" textAnchor="middle" fontSize="12" fontWeight="700" fill="#ffffff">EVENT</text>
                        <text x={x + 60} y="108" textAnchor="middle" fontSize="11" fill="#5d6b65">prev digest</text>
                        <rect x={x + 18} y="116" width="84" height="8" rx="4" fill={LINE} />
                        <text x={x + 60} y="144" textAnchor="middle" fontSize="11" fill="#5d6b65">payload</text>
                        <rect x={x + 18} y="150" width="84" height="8" rx="4" fill={GREEN} opacity="0.5" />
                        {i < blocks.length - 1 && (
                            <path d={`M${x + 120} 108 h32`} stroke={GREEN} strokeWidth="4" markerEnd="url(#arrow)" />
                        )}
                    </g>
                )
            })}
            <defs>
                <marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="5" markerHeight="5" orient="auto">
                    <path d="M0 0 L10 5 L0 10 z" fill={GREEN} />
                </marker>
            </defs>
            <text x="320" y="204" textAnchor="middle" fontSize="13" fill="#5d6b65">
                Sealed history cannot be edited; a replay that reorders is not a replay.
            </text>
        </Canvas>
    )
}

/**
 * The reasoning panel. Seventeen seats, the adversarial one marked, and the
 * execution seat drawn as absent — the node that reasons refuses to host it.
 */
export function PanelDiagram() {
    const seats = 17
    return (
        <Canvas label="A seventeen-seat reasoning panel with an adversarial reviewer and no execution seat" h={300}>
            <rect x="0" y="0" width="640" height="300" rx="18" fill={PAPER} />
            <ellipse cx="320" cy="150" rx="150" ry="72" fill="#ffffff" stroke={LINE} strokeWidth="2" />
            <text x="320" y="146" textAnchor="middle" fontSize="14" fontWeight="700" fill={INK}>THE PANEL</text>
            <text x="320" y="166" textAnchor="middle" fontSize="12" fill="#5d6b65">findings carry their evidence</text>
            {Array.from({ length: seats }, (_, i) => {
                const a = (i / seats) * Math.PI * 2 - Math.PI / 2
                const x = 320 + 208 * Math.cos(a)
                const y = 150 + 116 * Math.sin(a)
                const adversary = i === 10
                return (
                    <g key={i}>
                        <circle cx={x} cy={y} r={adversary ? 13 : 10}
                            fill={adversary ? GREEN : "#ffffff"} stroke={adversary ? GREEN : INK} strokeWidth="2" />
                        {adversary && (
                            <path d={`M${x - 5} ${y} h10 M${x} ${y - 5} v10`} stroke="#ffffff" strokeWidth="2.5"
                                strokeLinecap="round" transform={`rotate(45 ${x} ${y})`} />
                        )}
                    </g>
                )
            })}
            <text x="320" y="288" textAnchor="middle" fontSize="13" fill="#5d6b65">
                The marked seat is paid to disagree. There is no execution seat.
            </text>
        </Canvas>
    )
}

/** Quantum against its control group — the rule, not a result. */
export function BaselineDiagram() {
    return (
        <Canvas label="Every quantum run is paired with a classical baseline computed on the same problem" h={280}>
            <rect x="0" y="0" width="640" height="280" rx="18" fill={PAPER} />
            <rect x="248" y="26" width="144" height="44" rx="10" fill="#ffffff" stroke={INK} strokeWidth="2" />
            <text x="320" y="54" textAnchor="middle" fontSize="14" fontWeight="600" fill={INK}>One problem</text>
            <path d="M320 70 v22 M150 92 h340 M150 92 v26 M490 92 v26" fill="none" stroke={INK} strokeWidth="2" />
            <rect x="40" y="118" width="220" height="86" rx="12" fill="#ffffff" stroke={LINE} strokeWidth="2" />
            <rect x="40" y="118" width="6" height="86" rx="3" fill={INK} />
            <text x="64" y="148" fontSize="14" fontWeight="700" fill={INK}>Classical baseline</text>
            <text x="64" y="170" fontSize="12" fill="#5d6b65">computed every time,</text>
            <text x="64" y="188" fontSize="12" fill="#5d6b65">on the same inputs</text>
            <rect x="380" y="118" width="220" height="86" rx="12" fill="#ffffff" stroke={LINE} strokeWidth="2" />
            <rect x="380" y="118" width="6" height="86" rx="3" fill={GREEN} />
            <text x="404" y="148" fontSize="14" fontWeight="700" fill={INK}>Quantum run</text>
            <text x="404" y="170" fontSize="12" fill="#5d6b65">kept only where it beats</text>
            <text x="404" y="188" fontSize="12" fill="#5d6b65">the baseline, measurably</text>
            <path d="M150 204 v22 h340 v-22" fill="none" stroke={INK} strokeWidth="2" />
            <text x="320" y="252" textAnchor="middle" fontSize="14" fontWeight="700" fill={INK}>
                “We used a quantum computer” is not a result.
            </text>
        </Canvas>
    )
}

/** The console on a phone, as structure — no invented figures on the screen. */
export function ConsoleMockup() {
    const rows = [64, 92, 120, 148]
    return (
        <Canvas label="The Algorik console installed on a phone, showing a paper trading label" w={340} h={520}>
            <rect x="34" y="14" width="272" height="492" rx="36" fill={INK} />
            <rect x="46" y="26" width="248" height="468" rx="28" fill="#ffffff" />
            <rect x="136" y="34" width="68" height="10" rx="5" fill={INK} opacity="0.35" />
            <rect x="62" y="58" width="216" height="34" rx="8" fill={PAPER} />
            <rect x="70" y="68" width="82" height="14" rx="7" fill={GREEN} />
            <text x="111" y="79" textAnchor="middle" fontSize="9" fontWeight="700" fill="#ffffff">PAPER TRADING</text>
            <rect x="160" y="70" width="52" height="10" rx="5" fill={LINE} />
            {rows.map((y, i) => (
                <g key={y} transform={`translate(0 ${44 + i * 20})`}>
                    <rect x="62" y={y} width="216" height="52" rx="10" fill="#ffffff" stroke={LINE} strokeWidth="2" />
                    <rect x="74" y={y + 14} width={92 - i * 12} height="9" rx="4.5" fill={INK} opacity="0.7" />
                    <rect x="74" y={y + 31} width={140 - i * 16} height="7" rx="3.5" fill={LINE} />
                    <circle cx="258" cy={y + 26} r="7" fill={i === 1 ? GREEN : LINE} />
                </g>
            ))}
            <rect x="62" y="404" width="216" height="66" rx="10" fill={PAPER} />
            <path d="M74 452 L104 430 L134 440 L164 408 L194 424 L224 396 L262 412"
                fill="none" stroke={GREEN} strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" />
        </Canvas>
    )
}

/** What the loop refuses, shown as a funnel that mostly says no. */
export function FunnelDiagram() {
    const steps = [
        ["Candidates detected", 560],
        ["Survive scoring", 420],
        ["Reviewed by the panel", 300],
        ["Rehearsed in simulation", 200],
        ["Cleared by risk", 120],
    ]
    return (
        <Canvas label="Most candidates are discarded, and each discard is a recorded outcome" h={330}>
            <rect x="0" y="0" width="640" height="330" rx="18" fill={PAPER} />
            {steps.map(([label, w], i) => {
                const y = 26 + i * 56
                const x = (640 - w) / 2
                return (
                    <g key={label}>
                        <rect x={x} y={y} width={w} height={42} rx="10"
                            fill={i === steps.length - 1 ? GREEN : "#ffffff"} stroke={i === steps.length - 1 ? GREEN : LINE} strokeWidth="2" />
                        <text x="320" y={y + 27} textAnchor="middle" fontSize="14" fontWeight="600"
                            fill={i === steps.length - 1 ? "#ffffff" : INK}>{label}</text>
                    </g>
                )
            })}
            <text x="320" y="316" textAnchor="middle" fontSize="13" fill="#5d6b65">
                Every discard is a recorded outcome with a named reason — a quiet cycle is as auditable as a busy one.
            </text>
        </Canvas>
    )
}

/** Regional cells, each deciding inside its own envelope. */
export function CellsDiagram() {
    const cells = [
        [110, 120], [230, 78], [350, 96], [470, 74], [540, 158], [400, 190], [180, 208],
    ]
    return (
        <Canvas label="Seven regional cells, each trading inside its own capital envelope" h={280}>
            <rect x="0" y="0" width="640" height="280" rx="18" fill={PAPER} />
            {cells.map(([x, y], i) => (
                <g key={i}>
                    <circle cx={x} cy={y} r="34" fill="none" stroke={LINE} strokeWidth="2" strokeDasharray="4 5" />
                    <circle cx={x} cy={y} r="16" fill={i === 0 ? GREEN : "#ffffff"} stroke={i === 0 ? GREEN : INK} strokeWidth="2.5" />
                </g>
            ))}
            <text x="320" y="248" textAnchor="middle" fontSize="13" fontWeight="600" fill={INK}>
                A cell that cannot reach the centre keeps working inside its envelope.
            </text>
            <text x="320" y="268" textAnchor="middle" fontSize="12" fill="#5d6b65">
                The dashed ring is the bound. Nothing crosses it.
            </text>
        </Canvas>
    )
}
