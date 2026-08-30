import type { Metadata } from "next";
import Link from "next/link";
import { Bullets, LegalSection, LegalShell } from "../../ui";

export const metadata: Metadata = {
  title: "Risk Disclosures (draft)",
  description:
    "Draft risk disclosures for Algorik: simulated results do not predict real trading, no performance is promised, and the platform does not execute live trades.",
};

export default function RiskDisclosuresPage() {
  return (
    <LegalShell title="Risk Disclosures">
      <LegalSection heading="1. The three statements that matter most">
        <Bullets
          items={[
            <span key="simulated">
              <strong className="font-semibold">
                Simulated results do not predict real trading.
              </strong>{" "}
              Everything Algorik executes is simulated, and simulated performance has inherent
              limitations described below.
            </span>,
            <span key="performance">
              <strong className="font-semibold">No performance is promised.</strong> Algorik makes
              no representation that any strategy, model or output will achieve any result.
            </span>,
            <span key="live">
              <strong className="font-semibold">
                The platform does not currently execute live trades.
              </strong>{" "}
              It submits no orders to any live venue, and the boundary preventing it is enforced
              in the platform&apos;s infrastructure, its start-up checks and its type system.
            </span>,
          ]}
        />
      </LegalSection>

      <LegalSection heading="2. Limitations of simulated performance">
        <p>
          Simulated and hypothetical results are produced with the benefit of a controlled
          environment. Among other differences from live markets, a simulation:
        </p>
        <Bullets
          items={[
            "does not experience real liquidity constraints, and may assume fills at prices a live order would not have obtained;",
            "does not move the market — real orders create impact that simulation approximates at best;",
            "does not carry the operational risks of live execution, such as venue outages, rejected orders or partial connectivity;",
            "may be affected by data errors or revisions in the historical and streaming data it consumes, despite bitemporal recording designed to limit hindsight bias.",
          ]}
        />
        <p>
          A strategy that performs well in simulation may perform poorly, or fail entirely, under
          live conditions. No inference from simulated to live performance is safe, and Algorik
          does not invite one.
        </p>
      </LegalSection>

      <LegalSection heading="3. General investment risk">
        <p>
          Trading and investing in financial instruments involves substantial risk, including the
          possible loss of the entire amount invested. Markets are volatile; past performance —
          real or simulated — is not indicative of future results. No trading is without risk, and
          nothing on this platform changes that.
        </p>
      </LegalSection>

      <LegalSection heading="4. Not advice, not a solicitation">
        <p>
          Outputs of the platform — opportunities, findings, confidence figures, simulated fills
          and attribution — are research artefacts of a software system, not recommendations to
          you. Algorik does not provide investment, legal, accounting or tax advice, and nothing
          on this site or in the platform is an offer or solicitation to transact in any
          instrument.
        </p>
      </LegalSection>

      <LegalSection heading="5. Model and quantum research risk">
        <p>
          The platform&apos;s reasoning uses machine-learned and language models whose outputs can be
          wrong, and quantum optimisation methods that are experimental. Both are bounded by
          design — deterministic risk checks never route to a model, and every quantum experiment
          is measured against a classical baseline — but a bounded error is still an error, and
          research outputs should be treated accordingly.
        </p>
      </LegalSection>

      <LegalSection heading="6. If anything here changes">
        <p>
          Should the platform&apos;s execution posture ever change, this document would change
          first,
          with a new effective date and prominent notice — not as a quiet edit. Questions can be
          raised via the{" "}
          <Link className="underline underline-offset-2" href="/company#contact">
            contact page
          </Link>
          .
        </p>
      </LegalSection>
    </LegalShell>
  );
}
