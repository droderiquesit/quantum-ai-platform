"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { describeOutcome, type ApiOutcome } from "@/lib/api/client";
import {
  ledgerEligibility,
  type EligibilityDecisionBody,
  type EligibilityPermission,
  type LedgerUser,
} from "@/lib/hooks/useTreasury";

/**
 * The one control on the treasury surface: an operator's eligibility decision
 * about one user.
 *
 * What it records is a finding about a person — that an operator checked this
 * user's identity, on a date, against a document, in a jurisdiction, and that
 * the finding lapses on a date. It moves no money. There is no instrument,
 * side, quantity or price in its body, and no field about capital leaving the
 * platform: `Eligibility` in `qip-capital` omits `can_withdraw` rather than
 * always answering `false`, because a field is a value a transfer path could
 * one day read (ADR 0021, ADR 0023), and this form adds none.
 *
 * **Which operator is not part of that record.** The gateway
 * (`src/app/api/gateway/[...path]/route.ts`) authenticates to `qip-api` with
 * one deployment bearer token for every browser session — `upstreamHeaders`
 * in `src/lib/server/upstream.ts` is the whole of it — and the sealed
 * session's claims reach the platform nowhere, as `SessionClaims` in
 * `src/lib/server/identity.ts` says in as many words. `qip-api` builds every
 * credential's subject as `format!("{}@env", role.as_str())` in its `main.rs`,
 * and `routes.rs` hands that same `principal.subject` to
 * `Platform::decide_eligibility`. So the journaled decider is `operator@env`:
 * a deployment, identical for every human who signs in here. This panel
 * therefore names no person as the attributed decider. The copy read "Decide
 * eligibility as <name>" and "Confirm as <name>", and the attestation opened
 * "I, <name>, verified…"; each named an identity nothing downstream holds.
 * See the 2026-09-05 amendment to ADR 0041 for the same gap on the venue
 * registrations surface, the reasoning, and the design that would close it.
 * The signed-in name is still *displayed*, because who is at the keyboard is
 * a true fact about this console — it is simply not a fact the platform
 * records.
 *
 * Two things are deliberate about the shape of the control.
 *
 * It is a panel and not a hidden dialog, so a viewer sees the decision that
 * exists and sees it refused to them, with the reason, rather than seeing
 * nothing and concluding the platform has no such record. The whole panel is
 * a disabled `<fieldset>` in that case; a disabled control is not a control,
 * and forcing a click on it opens nothing.
 *
 * And the confirm step states the attestation in the first person — without a
 * name — because what is being recorded is a claim by whoever is at this
 * keyboard and not a form submission, while the name that would make it an
 * attribution is one the platform never receives. The document reference is
 * required before the confirm can be pressed: an eligibility whose evidence
 * nobody named is a check nobody can repeat. Every other refusal is the
 * platform's — an expiry that is not after the verification, a jurisdiction
 * that is not the mandate's, a blank timestamp — and each comes back as a 400
 * naming the field, rendered as it came.
 */

/** `yyyy-mm-dd` from a date input, or the empty string. */
type DateInput = string;

const NO_DATE = "(no date stated)";
const NO_DOCUMENT = "(no document stated)";

/**
 * The instant a stated date names, as the platform's RFC 3339.
 *
 * A date input carries no time, so a stated day is sent as that day at
 * midnight UTC and the field label says so. A blank date is sent blank rather
 * than filled in with today: the platform refuses it with a 400 naming the
 * field, which is the right party to refuse a fact nobody stated.
 */
function instantOf(date: DateInput): string {
  return date.length === 0 ? "" : `${date}T00:00:00Z`;
}

export function EligibilityPanel({
  user,
  operatorName,
  permission,
  onDecided,
}: {
  user: LedgerUser;
  /** The signed-in person, or `null` when nobody is. */
  operatorName: string | null;
  permission: EligibilityPermission;
  /** Called with the whole updated row the route answered. */
  onDecided: (row: LedgerUser) => void;
}) {
  const [decision, setDecision] = useState<"granted" | "revoked">("granted");
  const [verifiedOn, setVerifiedOn] = useState<DateInput>("");
  const [expiresOn, setExpiresOn] = useState<DateInput>("");
  const [canInvest, setCanInvest] = useState(false);
  // Prefilled from the mandate the platform answered, and editable. The
  // registry refuses an eligibility verified somewhere other than the
  // mandate's jurisdiction, so the mandate's value is the one an operator
  // almost always means — but it is the operator who states where they
  // checked, so it is not fixed.
  const [jurisdiction, setJurisdiction] = useState(user.mandate.jurisdiction);
  // Named `documentRef` rather than `document`: the DOM global of that name is
  // in scope in a client component, and a state variable that shadows it reads
  // as the document object to everyone who has not read this line.
  const [documentRef, setDocumentRef] = useState("");
  const [reviewing, setReviewing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<ApiOutcome<LedgerUser> | null>(null);
  // What the platform answered, and who was signed in when it was asked —
  // two separate facts, and the result line keeps them separate. The name is
  // this console's own knowledge and was never sent: the body below carries
  // no operator field, and the route would refuse one it did not read.
  const [recorded, setRecorded] = useState<{ signedInAs: string; verdict: string } | null>(null);

  const id = user.user_id;
  const documented = documentRef.trim().length > 0;

  // First person and no name: the sentence is what the person at this
  // keyboard asserts, and the platform records none of the name. "I, <name>,
  // verified…" read as the recorded attribution and there is no such record.
  const attestation = useMemo(() => {
    const evidence = documentRef.trim().length > 0 ? documentRef.trim() : NO_DOCUMENT;
    if (decision === "revoked") {
      return `I revoke this user's eligibility on the record ${evidence}; the ledger keeps the revoked record rather than removing it, so "never verified" and "verified and then revoked" stay different answers.`;
    }
    const verified = verifiedOn.length === 0 ? NO_DATE : verifiedOn;
    const expires = expiresOn.length === 0 ? NO_DATE : expiresOn;
    return `I verified this user's identity on ${verified} against ${evidence}; expires ${expires}`;
  }, [decision, documentRef, verifiedOn, expiresOn]);

  const body = useMemo<EligibilityDecisionBody>(
    () =>
      decision === "revoked"
        ? { decision: "revoked", reason: documentRef.trim() }
        : {
            decision: "granted",
            verified_at: instantOf(verifiedOn),
            can_invest: canInvest,
            jurisdiction: jurisdiction.trim(),
            expires_at: instantOf(expiresOn),
            reason: documentRef.trim(),
          },
    [decision, documentRef, verifiedOn, canInvest, jurisdiction, expiresOn],
  );

  const onSubmit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      if (operatorName === null) return;
      setBusy(true);
      setRefusal(null);
      try {
        const response = await ledgerEligibility.decide(id, body);
        if (response.outcome.kind === "ok") {
          const row = response.outcome.data;
          setRecorded({
            signedInAs: operatorName,
            verdict: row.eligibility?.eligible === true ? "eligible" : (row.eligibility?.refused ?? "not eligible"),
          });
          setReviewing(false);
          onDecided(row);
        } else {
          setRefusal(response.outcome);
        }
      } finally {
        setBusy(false);
      }
    },
    [operatorName, id, body, onDecided],
  );

  return (
    <div
      className="border-t border-[color:var(--color-line)] pt-2"
      data-testid={`eligibility-panel-${id}`}
      data-allowed={permission.allowed ? "true" : "false"}
    >
      <span className="eyebrow">decide eligibility</span>
      <fieldset disabled={!permission.allowed} className="mt-1 flex flex-col gap-2 border-0 p-0">
        <legend className="sr-only">Decide {id}&rsquo;s eligibility</legend>
        <div className="flex flex-wrap items-end gap-3">
          <div className="w-[130px]">
            <label className="field-label" htmlFor={`eligibility-decision-${id}`}>
              Decision
            </label>
            <select
              id={`eligibility-decision-${id}`}
              className="select"
              value={decision}
              onChange={(event) => setDecision(event.target.value === "revoked" ? "revoked" : "granted")}
              data-testid={`eligibility-decision-${id}`}
            >
              <option value="granted">granted</option>
              <option value="revoked">revoked</option>
            </select>
          </div>

          {decision === "granted" ? (
            <>
              <div className="w-[190px]">
                <label className="field-label" htmlFor={`eligibility-verified-${id}`}>
                  Verified on (UTC date; sent as 00:00:00Z)
                </label>
                <input
                  id={`eligibility-verified-${id}`}
                  type="date"
                  className="input"
                  value={verifiedOn}
                  onChange={(event) => setVerifiedOn(event.target.value)}
                  data-testid={`eligibility-verified-${id}`}
                />
              </div>
              <div className="w-[190px]">
                <label className="field-label" htmlFor={`eligibility-expires-${id}`}>
                  Expires on (every verification lapses)
                </label>
                <input
                  id={`eligibility-expires-${id}`}
                  type="date"
                  className="input"
                  value={expiresOn}
                  onChange={(event) => setExpiresOn(event.target.value)}
                  data-testid={`eligibility-expires-${id}`}
                />
              </div>
              <div className="w-[110px]">
                <label className="field-label" htmlFor={`eligibility-jurisdiction-${id}`}>
                  Verified in (alpha-2)
                </label>
                <input
                  id={`eligibility-jurisdiction-${id}`}
                  className="input"
                  value={jurisdiction}
                  onChange={(event) => setJurisdiction(event.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  data-testid={`eligibility-jurisdiction-${id}`}
                />
              </div>
              <label className="flex items-center gap-1.5 pb-1.5 text-[11.5px]" htmlFor={`eligibility-can-invest-${id}`}>
                <input
                  id={`eligibility-can-invest-${id}`}
                  type="checkbox"
                  checked={canInvest}
                  onChange={(event) => setCanInvest(event.target.checked)}
                  data-testid={`eligibility-can-invest-${id}`}
                />
                may have capital put to work
              </label>
            </>
          ) : null}
        </div>

        <div>
          <label className="field-label" htmlFor={`eligibility-document-${id}`}>
            Document reference — required; the evidence this finding rests on
          </label>
          <input
            id={`eligibility-document-${id}`}
            className="input"
            value={documentRef}
            onChange={(event) => setDocumentRef(event.target.value)}
            placeholder="e.g. passport GBR-…-2031, checked in person"
            autoComplete="off"
            required
            aria-required="true"
            data-testid={`eligibility-document-${id}`}
          />
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            className="btn"
            data-variant="primary"
            onClick={() => {
              setRefusal(null);
              setReviewing(true);
            }}
            disabled={!permission.allowed || !documented}
            aria-haspopup="dialog"
            data-testid={`eligibility-review-${id}`}
            title={permission.allowed ? undefined : permission.reason}
          >
            Decide eligibility
</button>
          {permission.allowed && !documented ? (
            <span
              className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]"
              data-testid={`eligibility-document-required-${id}`}
            >
              name the document first: a finding whose evidence nobody recorded is a check nobody
              can repeat
            </span>
          ) : null}
        </div>
      </fieldset>

      {permission.allowed ? null : (
        <p
          className="mt-1 text-[11px] leading-snug text-[color:var(--color-warn)]"
          data-testid={`eligibility-permission-${id}`}
        >
          {permission.reason}
        </p>
      )}

      {recorded === null ? null : (
        <p
          className="mt-1 text-[11px] leading-snug text-[color:var(--color-ink-dim)]"
          data-testid={`eligibility-result-${id}`}
        >
          Decided while signed in as <span className="num">{recorded.signedInAs}</span>, which this
          console knows and the platform was not told: the record attributes the decision to this
          console&rsquo;s own credential subject, the same for every person who signs in here. The
          platform answered: <span className="num">{recorded.verdict}</span>.
        </p>
      )}

      {reviewing && operatorName !== null ? (
        <ConfirmDialog
          userId={id}
          attestation={attestation}
          operatorName={operatorName}
          busy={busy}
          refusal={refusal}
          onClose={() => setReviewing(false)}
          onSubmit={onSubmit}
        />
      ) : null}
    </div>
  );
}

/**
 * The confirm step.
 *
 * Two clicks and a sentence in the first person, because the record is a
 * claim by a person rather than the output of a form — and a paragraph, before
 * the click rather than discovered afterwards in the record, saying that the
 * person the platform will name is this console's deployment credential and
 * not the one signing. The refusals are shown
 * as they came: the gateway's own 405 is distinguished from the platform's
 * 403, a 400 is repeated verbatim because it names the field to correct, and
 * a 409 means the operator identity has aged out of the kernel's fifteen
 * minutes, whose remedy is a fresh sign-in and not a retry.
 */
function ConfirmDialog({
  userId,
  attestation,
  operatorName,
  busy,
  refusal,
  onClose,
  onSubmit,
}: {
  userId: string;
  attestation: string;
  operatorName: string;
  busy: boolean;
  refusal: ApiOutcome<LedgerUser> | null;
  onClose: () => void;
  onSubmit: (event: React.FormEvent<HTMLFormElement>) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="w-[min(640px,92vw)] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] p-0 text-[color:var(--color-ink)] backdrop:bg-black/70"
      aria-labelledby="eligibility-confirm-title"
      onClose={onClose}
      onCancel={onClose}
      data-testid="eligibility-dialog"
    >
      <div className="border-b border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-4 py-2.5">
        <h2 id="eligibility-confirm-title" className="panel-title">
          Confirm the eligibility decision for {userId}
        </h2>
      </div>
      <form onSubmit={onSubmit} className="flex flex-col gap-3 px-4 py-4">
        <p className="text-[12px] leading-relaxed" data-testid="eligibility-attestation">
          {attestation}
        </p>
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          Confirming sends{" "}
          <code className="num">POST /api/v1/ledger/users/{userId}/eligibility</code>. The platform
          records the operator from its own credential, refuses a credential without the operator
          role with a 403, refuses a malformed or missing term with a 400 naming the field, and
          refuses an operator identity older than fifteen minutes with a 409. The decision moves no
          capital and names no instrument; it states who was verified, when, where and against what.
        </p>
        {/*
          The attribution gap, stated where the click is made. This console
          authenticates to the platform with one deployment credential shared
          by every browser session, and the platform has no way to verify a
          console session's identity, so the subject it journals is that
          credential's — not the person at the keyboard. Saying "Confirm as
          <name>" here named an identity nothing downstream records. The venue
          registrations dialog carries the same paragraph for the same reason;
          see the 2026-09-05 amendment to ADR 0041.
        */}
        <p
          className="text-[11.5px] leading-relaxed text-[color:var(--color-warn)]"
          data-testid="eligibility-attribution"
        >
          What the platform records is this console&apos;s own API credential — a deployment
          subject such as <span className="num">operator@env</span>, the same for every person who
          signs in here — and not {operatorName}. That you are the one deciding is known only from
          this console&apos;s sign-in record, which the platform&apos;s event log does not hold.
          Confirm only if you personally checked this identity against the document named above.
        </p>
        {refusal === null ? null : (
          <p
            role="alert"
            className="text-[11.5px] leading-relaxed text-[color:var(--color-down)]"
            data-testid="eligibility-refusal"
          >
            {refusal.kind === "denied"
              ? `The platform refused this console's credential (${refusal.status}): ${refusal.detail}`
              : refusal.kind === "error" && refusal.status === 409
                ? `your session's credential is older than 15 minutes; sign in again (409: ${refusal.detail})`
                : describeOutcome(refusal)}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button type="button" className="btn" data-variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            type="submit"
            className="btn"
            data-variant="primary"
            disabled={busy}
            data-testid="eligibility-confirm"
          >
            {busy ? "Recording…" : "Confirm decision"}
          </button>
        </div>
      </form>
    </dialog>
  );
}
