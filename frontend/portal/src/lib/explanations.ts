import { NOT_YET_SERVED, type MissingEndpoint } from "@/lib/api/endpoints";

/**
 * Blueprint §40.2's seven explanation questions and what this console can
 * answer to each, declared from the route contract.
 *
 * A plain module with no React in it, because
 * `tests/intelligence-explanations.spec.ts` imports the table under Node to
 * assert the page against it, and the page that renders it is a client
 * component that would drag `next/link` in. The reasoning per question is in
 * the page's module note (`src/app/(portal)/intelligence/explanations/page.tsx`).
 */

/** What the platform can say to a question, declared from the route contract. */
export type Coverage = "answered" | "partial" | "absent";

export interface Question {
  readonly id: string;
  /** The blueprint's wording, verbatim. */
  readonly asks: string;
  /** The blueprint's "what answers it" cell, verbatim. */
  readonly answeredBy: string;
  readonly coverage: Coverage;
  /** The routes read for the live panel, by path under /api/v1. */
  readonly reads: readonly string[];
  /** The console page that renders the answering route in full, if one does. */
  readonly page: { readonly href: string; readonly label: string } | null;
  /** The declared absence, where nothing answers. */
  readonly missing: MissingEndpoint | null;
}

/**
 * The seven, in the blueprint's order. `coverage` is transcribed from the
 * page's module note; a route that starts answering a question is an edit
 * here and to the panel that renders it, in that order.
 */
export const QUESTIONS: readonly Question[] = [
  {
    id: "position",
    asks: "Why did you take this position?",
    answeredBy: "The belief that supported it, its confidence, and the evidence that formed it",
    // Half. `GET /proposals` now carries, per leg, the hypotheses that leg
    // expresses — by id — beside the proposal's rationale. That is the
    // belief the position rests on, named. Its *confidence* and the evidence
    // that formed it are still on no wire: a hypothesis id is a handle, not
    // an argument, and no route resolves one.
    coverage: "partial",
    // Empty on purpose. `reads` is what *this* page fetches for its live
    // panel, and it fetches no proposals; the route that carries the answer
    // is named through `page`, which is what that field is for. Declaring a
    // route here that the page never requests would be a claim nobody could
    // check from the network tab.
    reads: [],
    page: { href: "/intelligence/decisions", label: "Decision record" },
    missing: null,
  },
  {
    id: "belief",
    asks: "Why do you believe that?",
    answeredBy: "The causal path through the graph, and the episodes that resemble now",
    coverage: "partial",
    reads: ["/cognition/precedents"],
    page: { href: "/cognition/precedents", label: "Precedents" },
    missing: null,
  },
  {
    id: "size",
    asks: "Why this size?",
    answeredBy: "Edge, volatility, grant, and the confidence multiplier, shown separately",
    // Half, and the half is named rather than rounded up. `GET /proposals`
    // now projects the sizing the DECIDE stage recorded: the weight each leg
    // moved from and to, the reference price it was sized at, the cost in
    // basis points, and the optimiser's own sentences about what the result
    // gave up — which name the caps that bound it and the evidence a bound
    // was narrowed on. That is the sizing *as it happened*.
    //
    // It is not the blueprint's four terms shown separately. Edge,
    // volatility, the grant and the confidence multiplier are inputs to the
    // optimiser and are not projected apart by any route. A page could not
    // honestly derive them from a weight, and would be a second sizing model
    // in a browser if it tried.
    coverage: "partial",
    // Empty on purpose. `reads` is what *this* page fetches for its live
    // panel, and it fetches no proposals; the route that carries the answer
    // is named through `page`, which is what that field is for. Declaring a
    // route here that the page never requests would be a claim nobody could
    // check from the network tab.
    reads: [],
    page: { href: "/intelligence/decisions", label: "Decision record" },
    missing: null,
  },
  {
    id: "declined",
    asks: "Why not the obvious trade?",
    answeredBy: "The gate that declined it and the counterfactual score of declining it",
    // Still half, and for a different reason than before. The *gate* half is
    // now answered per decision rather than only per rule: a vetoed proposal
    // on `GET /proposals` names the control that vetoed it and the reason it
    // gave, which the route previously dropped — a console could report
    // `vetoed` and not say by what. The counterfactual score of declining is
    // still aggregated per rule over a window on `/risk/recalibrations`, not
    // attached to the decision, so half a question remains half.
    coverage: "partial",
    reads: ["/risk/recalibrations", "/orders"],
    page: { href: "/intelligence/decisions", label: "Decision record" },
    missing: null,
  },
  {
    id: "selection",
    asks: "Why this strategy and not that one?",
    answeredBy: "The optimisation run, the objective, and the correlation that ruled the other out",
    coverage: "absent",
    reads: [],
    page: null,
    missing: NOT_YET_SERVED["explanationSelection"] ?? null,
  },
  {
    id: "unknowns",
    asks: "What do you not know here?",
    answeredBy: "The self-model — stated coverage gaps and unreliable estimates",
    coverage: "answered",
    reads: ["/cognition/self-model"],
    page: { href: "/cognition/self-model", label: "Self-model" },
    missing: null,
  },
  {
    id: "cost",
    asks: "Is this platform sensible at my capital?",
    answeredBy: "Total cost against attributed return, stated plainly",
    coverage: "partial",
    reads: ["/models", "/pnl"],
    page: { href: "/portfolio/pnl", label: "P&L & attribution" },
    missing: null,
  },
];
