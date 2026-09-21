import Link from "next/link";
import { headers } from "next/headers";
import { safeRedirect } from "@algorik/auth";
import { iapConfigured, iapIdentityFrom } from "@/lib/server/iap";
import SignInCard from "./sign-in-form";

/**
 * Sign in — a server component, because the first question is one only the
 * server can answer.
 *
 * **Google sign-in on this console is Identity-Aware Proxy's, and there is
 * deliberately no second one.** ADR 0094 puts the portal behind IAP at
 * `portal.algorik.ai`; a request that reaches this process has already been
 * authenticated by Google against a Google account and refused if that account
 * is not on the access list. Running an OAuth dance of our own here would send
 * a person who has just proved a Google identity away to prove it again, and
 * would need a browser-delivered client id that ADR 0094 decision 2 arranged
 * for there not to be — it chose the Google-managed OAuth client precisely so
 * the deployment "mints, stores and rotates nothing". So this page does not
 * offer a button; it reports the identity already proved, and the person
 * carries on.
 *
 * The assertion's ECDSA signature is verified before a single character of
 * this page is chosen — see `lib/server/iap.ts` for why a header read without
 * that check is a header an attacker sets.
 *
 * Three states, all real, none of them a lie about the deployment:
 *
 * 1. **A verified assertion** — the identity, and a way onward. No form.
 * 2. **IAP configured, no valid assertion** — somebody reached the origin
 *    without passing the proxy. The password form, and a caption that says so
 *    rather than "Google is not configured", which would be false.
 * 3. **No IAP at all** — local development and the Playwright suites. The
 *    password form exactly as it was.
 */
export const dynamic = "force-dynamic";

export default async function SignInPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const identity = await iapIdentityFrom(await headers());
  if (!identity) return <SignInCard iapConfigured={iapConfigured()} />;

  const nextParameter = (await searchParams).next;
  // The same `safeRedirect` the password form uses. A `next` honoured
  // verbatim is an open redirect wearing our sign-in page, and it is no less
  // one for the person having arrived through IAP.
  const destination = safeRedirect(typeof nextParameter === "string" ? nextParameter : null, "/");

  return (
    <div className="flex flex-col gap-4" data-testid="iap-signed-in">
      {/* The same markup `AuthHeading` renders, inlined rather than imported:
          `_lib/forms.tsx` is a client module, and pulling it in from here
          would ship the whole password form to a browser that is being told
          it does not need one. */}
      <header>
        <h1 className="text-[15px] font-semibold text-[color:var(--color-ink)]">
          Signed in with Google
        </h1>
        <p className="mt-1 text-[12px] leading-snug text-[color:var(--color-ink-dim)]">
          Research console access. Paper trading only.
        </p>
      </header>

      <div className="border border-[color:var(--color-border)] p-3">
        <p className="text-[11px] uppercase tracking-wide text-[color:var(--color-ink-faint)]">
          Identity asserted by Identity-Aware Proxy
        </p>
        <p data-testid="iap-email" className="mt-1 text-[13px] text-[color:var(--color-ink)]">
          {identity.email}
        </p>
        {identity.hostedDomain ? (
          <p className="mt-1 text-[11px] text-[color:var(--color-ink-dim)]">
            Workspace domain {identity.hostedDomain}
          </p>
        ) : null}
      </div>

      {/* Not decoration. A person who can see this page can see it because
          Google authenticated them and an operator put them on the IAP access
          list; what they may *do* here is a separate decision this console
          makes, and it grants the viewer role and nothing else. Saying so on
          the page is cheaper than someone inferring that Google's sign-in
          carried an entitlement with it. */}
      <p className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">
        Verified upstream by Google. This console did not ask for a second
        credential and has none to ask for. Access is read-only — the viewer
        role — regardless of the account; elevation is an operator decision
        with a record.
      </p>

      <Link href={destination} className="btn w-full text-center" data-testid="iap-continue">
        Continue to the console
      </Link>
    </div>
  );
}
