import type { Metadata } from "next";
import Link from "next/link";
import { Bullets, LegalSection, LegalShell } from "../../ui";

export const metadata: Metadata = {
  title: "Terms of Service (draft)",
  description:
    "Draft terms of service for Algorik, a paper-trading research platform. Not yet reviewed by counsel and not in effect.",
};

export default function TermsPage() {
  return (
    <LegalShell title="Terms of Service">
      <LegalSection heading="1. What these terms cover">
        <p>
          These terms will govern access to and use of Algorik (the &ldquo;Service&rdquo;), a
          software platform for investment research operated in paper-trading mode. By creating an
          account or using the Service, you agree to be bound by the version of these terms in
          effect at the time of use.
        </p>
      </LegalSection>

      <LegalSection heading="2. The Service is simulated">
        <p>
          Algorik executes exclusively against simulators and sandboxes. The Service does not
          submit orders to any live trading venue, does not hold client funds or securities, and
          provides no facility for doing either. Figures shown as fills, positions or
          profit-and-loss within the Service are the outputs of simulation and are labelled as
          such.
        </p>
      </LegalSection>

      <LegalSection heading="3. No investment advice; no brokerage">
        <p>
          Nothing in the Service constitutes investment advice, a recommendation, an offer, or a
          solicitation to buy or sell any financial instrument. Algorik is not acting as your
          broker, dealer, adviser or fiduciary. You are solely responsible for any investment
          decision you make outside the Service, and you should obtain independent professional
          advice before making one.
        </p>
      </LegalSection>

      <LegalSection heading="4. Accounts">
        <Bullets
          items={[
            "You must provide accurate registration information and keep your credentials confidential.",
            "You are responsible for activity under your account until you notify us of unauthorised use.",
            "We may suspend or terminate an account that breaches these terms, with notice where reasonably practicable.",
          ]}
        />
      </LegalSection>

      <LegalSection heading="5. Acceptable use">
        <p>You agree not to:</p>
        <Bullets
          items={[
            "attempt to access another user's data or any system component not intentionally exposed to you;",
            "probe, disable or circumvent security or rate-limiting controls, including the paper-trading boundary;",
            "use the Service to violate applicable law, including market abuse and data-protection law;",
            "misrepresent outputs of the Service as live trading results.",
          ]}
        />
      </LegalSection>

      <LegalSection heading="6. Intellectual property">
        <p>
          The Service, including its software, design system, marks and documentation, remains the
          property of Algorik and its licensors. You retain rights in content you submit; you
          grant us the licence needed to operate the Service on it, and no more.
        </p>
      </LegalSection>

      <LegalSection heading="7. Disclaimers">
        <p>
          The Service is provided &ldquo;as is&rdquo; and &ldquo;as available&rdquo;, without
          warranties of any kind, express or implied, including fitness for a particular purpose
          and non-infringement. Simulated results do not predict real trading outcomes, and no
          outcome of any kind is promised. See also the{" "}
          <Link className="underline underline-offset-2" href="/legal/risk-disclosures">
            Risk Disclosures
          </Link>
          .
        </p>
      </LegalSection>

      <LegalSection heading="8. Limitation of liability">
        <p>
          To the maximum extent permitted by law, Algorik will not be liable for indirect,
          incidental, special, consequential or punitive damages, or for loss of profits, data or
          goodwill, arising from use of the Service. Nothing in these terms excludes liability
          that cannot be excluded by law.
        </p>
      </LegalSection>

      <LegalSection heading="9. Changes and termination">
        <p>
          We may change the Service or these terms. Material changes will be published with a new
          effective date before they apply. You may stop using the Service at any time; sections
          which by their nature survive termination will survive it.
        </p>
      </LegalSection>

      <LegalSection heading="10. Governing law and contact">
        <p>
          Governing law and venue are to be determined before these terms take effect. Questions
          about this draft can be sent via the{" "}
          <Link className="underline underline-offset-2" href="/company#contact">
            contact page
          </Link>
          .
        </p>
      </LegalSection>
    </LegalShell>
  );
}
