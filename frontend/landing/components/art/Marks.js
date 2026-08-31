/**
 * The five coverage marks.
 *
 * The template shipped grey 206x211 boxes with their own pixel dimensions
 * printed on them — literal placeholders, not artwork. They rendered as grey
 * rectangles on the live home page. These replace them with drawings that
 * carry no number a reader could mistake for a quote, because a landing page
 * that invents a price is the first thing a visitor can check and disprove.
 */

const STROKE = "#10b981"
const INK = "#131615"

function Frame({ children, label }) {
    return (
        <svg viewBox="0 0 206 211" width="206" height="211" role="img" aria-label={label}
            style={{ maxWidth: "100%", height: "auto" }}>
            <rect x="0.5" y="0.5" width="205" height="210" rx="14" fill="#f4f8f6" stroke="#e2ece7" />
            {children}
        </svg>
    )
}

/** Equities — a bar series with a session divider, no values printed. */
export function EquitiesMark() {
    const bars = [
        [34, 118, 18, 54], [58, 96, 18, 76], [82, 132, 18, 40],
        [106, 78, 18, 94], [130, 110, 18, 62], [154, 62, 18, 110],
    ]
    return (
        <Frame label="Equities">
            <line x1="26" y1="176" x2="180" y2="176" stroke="#cfe0d8" strokeWidth="2" />
            {bars.map(([x, y, w, h], i) => (
                <rect key={x} x={x} y={y} width={w} height={h} rx="3"
                    fill={i % 2 === 0 ? STROKE : INK} opacity={i % 2 === 0 ? 1 : 0.75} />
            ))}
            <path d="M26 66 L188 66" stroke="#cfe0d8" strokeWidth="1" strokeDasharray="4 6" />
        </Frame>
    )
}

/** Currencies — two exchanged legs around a settlement node. */
export function CurrenciesMark() {
    return (
        <Frame label="Currencies">
            <circle cx="103" cy="105" r="46" fill="none" stroke={INK} strokeWidth="2" opacity="0.75" />
            <path d="M64 88 H142 l-16 -14" fill="none" stroke={STROKE} strokeWidth="6"
                strokeLinecap="round" strokeLinejoin="round" />
            <path d="M142 124 H64 l16 14" fill="none" stroke={INK} strokeWidth="6"
                strokeLinecap="round" strokeLinejoin="round" />
            <circle cx="103" cy="105" r="66" fill="none" stroke="#cfe0d8" strokeWidth="1" strokeDasharray="3 7" />
        </Frame>
    )
}

/** Crypto and on-chain — linked blocks, one of them a reorg branch. */
export function ChainMark() {
    return (
        <Frame label="Crypto and on-chain">
            {[26, 74, 122].map((x) => (
                <rect key={x} x={x} y="76" width="38" height="38" rx="7" fill={INK} opacity="0.85" />
            ))}
            <rect x="146" y="76" width="38" height="38" rx="7" fill={STROKE} />
            <line x1="64" y1="95" x2="74" y2="95" stroke={STROKE} strokeWidth="4" />
            <line x1="112" y1="95" x2="122" y2="95" stroke={STROKE} strokeWidth="4" />
            <line x1="160" y1="95" x2="170" y2="95" stroke={STROKE} strokeWidth="4" opacity="0" />
            {/* the discarded fork: drawn, because reorgs are handled rather than assumed away */}
            <path d="M141 95 q14 0 14 34" fill="none" stroke="#b8ccc3" strokeWidth="3" strokeDasharray="5 5" />
            <rect x="136" y="132" width="38" height="30" rx="7" fill="none" stroke="#b8ccc3" strokeWidth="2" strokeDasharray="5 5" />
        </Frame>
    )
}

/** Commodities — a forward curve in contango over its delivery ticks. */
export function CommoditiesMark() {
    return (
        <Frame label="Commodities">
            <line x1="26" y1="160" x2="182" y2="160" stroke="#cfe0d8" strokeWidth="2" />
            {[42, 70, 98, 126, 154].map((x) => (
                <line key={x} x1={x} y1="160" x2={x} y2="168" stroke="#cfe0d8" strokeWidth="2" />
            ))}
            <path d="M42 132 C 70 112, 98 96, 154 62" fill="none" stroke={STROKE} strokeWidth="5" strokeLinecap="round" />
            {[[42, 132], [70, 112], [98, 96], [126, 78], [154, 62]].map(([cx, cy]) => (
                <circle key={cx} cx={cx} cy={cy} r="6" fill="#ffffff" stroke={INK} strokeWidth="3" />
            ))}
        </Frame>
    )
}

/** Macro and rates — a term structure with a knowable-at marker. */
export function MacroMark() {
    return (
        <Frame label="Macro and rates">
            <line x1="26" y1="150" x2="182" y2="150" stroke="#cfe0d8" strokeWidth="2" />
            <path d="M30 128 C 70 128, 96 74, 178 66" fill="none" stroke={INK} strokeWidth="5"
                strokeLinecap="round" opacity="0.8" />
            <path d="M30 142 C 74 142, 104 106, 178 96" fill="none" stroke={STROKE} strokeWidth="5"
                strokeLinecap="round" strokeDasharray="10 7" />
            <line x1="104" y1="46" x2="104" y2="160" stroke="#b8ccc3" strokeWidth="2" strokeDasharray="4 5" />
            <circle cx="104" cy="92" r="7" fill={STROKE} stroke="#ffffff" strokeWidth="3" />
        </Frame>
    )
}

export const COVERAGE_MARKS = {
    equities: EquitiesMark,
    currencies: CurrenciesMark,
    chain: ChainMark,
    commodities: CommoditiesMark,
    macro: MacroMark,
}
