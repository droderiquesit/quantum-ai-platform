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
    coverage: "absent",
    reads: [],
    page: null,
    missing: NOT_YET_SERVED["explanationPosition"] ?? null,
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
    coverage: "absent",
    reads: [],
    page: null,
    missing: NOT_YET_SERVED["explanationSizing"] ?? null,
  },
  {
    id: "declined",
    asks: "Why not the obvious trade?",
    answeredBy: "The gate that declined it and the counterfactual score of declining it",
    coverage: "partial",
    reads: ["/risk/recalibrations", "/orders"],
    page: { href: "/risk/recalibrations", label: "Rule recalibrations" },
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
