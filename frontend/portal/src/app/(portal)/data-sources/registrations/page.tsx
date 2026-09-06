"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { describeOutcome, platform, type ApiOutcome } from "@/lib/api/client";
import type { SystemStatus } from "@/lib/api/types";
import { formatTimestamp } from "@/lib/format";
import {
  approvalPermission,
  registrations,
  slotAccess,
  useRegistrations,
  useRegistrationSlots,
  useSessionIdentity,
  viewerStanding,
  SLOTS_REFUSAL,
  SLOTS_ROUTE,
  type Approval,
  type RegistrationSlotSource,
  type RegistrationSource,
  type SessionIdentity,
  type SlotAccess,
} from "@/lib/hooks/useRegistrations";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Venue registrations: one card per source, and the one approval an operator
 * records.
 *
 * The request that produced the platform's registration registry was for a
 * scraper that signs up for venue APIs on its own, anonymously. The platform
 * refuses that (`docs/operations/registering-a-venue.md`), and this page is
 * the honest counterpart: it shows what each venue demands, who has
 * registered, the terms the operator is told to read, the deployment variable
 * the credential lives under and the command that puts it there — and, for a
 * source still pending, a control by which a signed-in operator records that
 * a registration was made under terms they read. Approving here creates no
 * account and reads no venue: it is a record of a fact a person made true.
 *
 * What that record does *not* carry is which person. The gateway
 * (`src/app/api/gateway/[...path]/route.ts`) authenticates to `qip-api` with
 * one deployment bearer token for every browser session, and the sealed
 * session's claims reach the platform nowhere — `src/lib/server/identity.ts`
 * says so in as many words — so `principal.subject`, and therefore the
 * journaled operator, is the console's credential subject (`operator@env`).
 * This page therefore names no person as the attributed operator: the confirm
 * step states what is actually recorded. The copy said "Approve registration
 * as <name>" and "Confirm as <name>", which named an identity nothing
 * downstream holds. See the 2026-09-05 amendment to ADR 0041 for the gap and
 * the design that would close it.
 *
 * The control is refused before the platform is asked when the session does
 * not hold the operator role, with the reason beside it, and the platform's
 * own refusals — 403 for a credential without the role, 400 naming the field
 * for a blank or key-shaped secret — are rendered as they came. The secret
 * field takes a variable *name* and the page says so; a pasted key is what
 * the platform's 400 exists to catch, and it is not repeated back.
 *
 * **Two reads, and the second one is usually refused.** The slot a credential
 * is read under moved off `GET /registrations` onto `GET /registrations/slots`
 * at the operator role, because a slot names where a credential lives in this
 * deployment's secret store and it was being served to a viewer credential.
 * This console's own credential is the viewer token (ADR 0018), so the slot
 * rows here render the platform's refusal, naming who may read the route and
 * how, rather than an empty cell. An empty cell is the failure that matters:
 * `secret_slot: null` already means something specific on this page — the
 * manifest names no variable, so there is nothing to prefill — and a blank
 * left by a refusal would be read as that, telling an operator no credential
 * is needed for a source the platform is refusing for want of one.
 *
 * Nothing here submits an order or could. The one write names a source and a
 * terms reference; no instrument, side, quantity or price exists on this page.
 */
export default function RegistrationsPage() {
  const resource = useRegistrations();
  const slotsResource = useRegistrationSlots();
  const access = slotAccess(slotsResource);
  const identity = useSessionIdentity();
  const permission = approvalPermission(identity);
  const operatorName = identity.status === "authenticated" ? identity.name : null;

  // The standing the platform answered to an approval, shown on the card
  // until the next GET lands. The platform's own list wins on every refresh:
  // a page that kept its local copy over the platform's would be the page
  // that showed "registered" the day the platform's record was lost.
  const [approved, setApproved] = useState<ReadonlyMap<string, Approval>>(new Map());
  const lastReceived = useRef<number | null>(resource.receivedAt);
  useEffect(() => {
    if (resource.receivedAt !== lastReceived.current) {
      lastReceived.current = resource.receivedAt;
      setApproved(new Map());
    }
  }, [resource.receivedAt]);

  /**
   * The card being confirmed, with whatever slot row the operator list
   * supplied for it — `null` when that read was refused, which is what makes
   * the dialog's variable field start empty rather than prefilled with a name
   * this console never received.
   */
  const [confirming, setConfirming] = useState<{
    readonly source: RegistrationSource;
    readonly slot: RegistrationSlotSource | null;
  } | null>(null);

  const refresh = resource.refresh;
  // The slots read is not polled, so an approval is the one event that has to
  // ask it again: the `secret` a record names is the only field on that route
  // an approval changes.
  const refreshSlots = slotsResource.refresh;
  const onApproved = useCallback(
    (approval: Approval) => {
      setApproved((current) => new Map(current).set(approval.source_id, approval));
      setConfirming(null);
      refresh();
      refreshSlots();
    },
    [refresh, refreshSlots],
  );

  return (
    <div className="flex flex-col gap-3 p-3">
      <RegistrationsHeader
        posture={resource.data?.posture ?? null}
        servedAt={resource.data?.served_at ?? null}
        identity={identity}
        meta={<Freshness resource={resource} name="registrations" />}
      />

      <ResourceView resource={resource} loadingRows={4}>
        {(data) =>
          data.sources.length === 0 ? (
            <Panel>
              <PanelBody>
                <EmptyBlock headline="The platform declares no source's registration requirement.">
                  <p>
                    An absent requirement is a question nobody asked, not a source that needs no
                    key; the platform refuses such a source as unknown. Nothing is listed here
                    because the platform listed nothing.
                  </p>
                </EmptyBlock>
              </PanelBody>
            </Panel>
          ) : (
            <div className="grid grid-cols-1 gap-3 xl:grid-cols-2" data-testid="registration-cards">
              {data.sources.map((listed) => {
                const approval = approved.get(listed.source_id);
                // The approval answers the operator shape; this card is built
                // from the viewer's list, so the standing is narrowed the way
                // the platform narrows it rather than spread across.
                const source =
                  approval === undefined
                    ? listed
                    : { ...listed, standing: viewerStanding(approval.standing) };
                const slot =
                  access.status === "available" ? access.bySource.get(source.source_id) ?? null : null;
                return (
                  <SourceCard
                    key={source.source_id}
                    source={source}
                    slot={slot}
                    access={access}
                    permission={permission}
                    onApprove={() => setConfirming({ source, slot })}
                  />
                );
              })}
            </div>
          )
        }
      </ResourceView>

      {confirming === null || operatorName === null ? null : (
        <ApproveDialog
          source={confirming.source}
          slot={confirming.slot}
          operatorName={operatorName}
          onClose={() => setConfirming(null)}
          onApproved={onApproved}
        />
      )}
    </div>
  );
}

/**
 * The declaration on the page and not only in the chrome, with the body's own
 * `posture` literal rendered as it came and the platform's live capability
 * read from `GET /system/status`, exactly as the treasury pages do it.
 *
 * `served_at` is shown beside the freshness chip because they are two
 * different facts and only one of them is the platform's: the chip is when
 * this browser received an answer, `served_at` is the instant the platform
 * says it answered. A console that showed only the first would report a
 * cached or replayed body as current.
 */
function RegistrationsHeader({
  posture,
  servedAt,
  identity,
  meta,
}: {
  posture: string | null;
  servedAt: string | null;
  identity: SessionIdentity;
  meta: ReactNode;
}) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "registrations-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="registrations-header">
      <PanelHead
        title="Venue registrations"
        meta={meta}
        actions={
          <>
            {posture === null ? null : (
              <Chip tone={posture === "PAPER TRADING" ? "ok" : "bad"} title="GET /registrations: posture">
                <span data-testid="registrations-body-posture">{posture}</span>
              </Chip>
            )}
            {status.data === null ? null : (
              <StatusChip
                tone={status.data.live_capable ? "bad" : "ok"}
                label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
                title="GET /system/status: live_capable"
              />
            )}
          </>
        }
      />
      <PanelBody>
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]" data-testid="registrations-declaration">
          <span className="chip mr-2" data-tone="ok" data-testid="registrations-paper-label">
            PAPER TRADING
          </span>
          Nothing on this page reads a venue, creates an account or submits an order. It reads{" "}
          <span className="num">GET /registrations</span> for what each venue demands and where each
          source stands, and <span className="num">GET /registrations/slots</span> for the
          deployment variable behind each — the second at the operator role, which this
          console&apos;s own credential does not hold, so its rows say so rather than showing
          nothing. Both are rendered as the platform answered them.
          Its one control records that a venue was registered under terms someone read, and the
          platform will not create the account anonymously: a source that needs an account stays
          refused until that record exists, and the record cannot be made without a person
          clicking. The operator the platform journals is this console&apos;s own API credential,
          identical for every session signed in here — the record names the deployment that acted,
          not which person clicked.
        </p>
        {servedAt === null ? null : (
          <p className="mt-2">
            <Muted testId="registrations-served-at">served {formatTimestamp(servedAt)}</Muted>
          </p>
        )}
        <p className="mt-2 text-[11px] leading-snug text-[color:var(--color-ink-faint)]" data-testid="registrations-session">
          {identity.status === "loading"
            ? "reading who is signed in…"
            : identity.status === "unauthenticated"
              ? "no one is signed in to this console; approvals need a named operator"
              : `signed in as ${identity.name} (${identity.email}), roles: ${identity.roles.length === 0 ? "none" : identity.roles.join(", ")}`}
        </p>
      </PanelBody>
    </Panel>
  );
}

const REQUIREMENT_LABEL: Record<string, string> = {
  keyless: "keyless — a public endpoint, no account",
  self_service_api_key: "self-service API key, under a person's account",
  account: "an account with the venue, in the operator's own name",
  account_with_identity_verification: "an account that passed the venue's identity verification",
};

function SourceCard({
  source,
  slot,
  access,
  permission,
  onApprove,
}: {
  source: RegistrationSource;
  /** This source's row on the operator list, or `null` when that read did not land. */
  slot: RegistrationSlotSource | null;
  access: SlotAccess;
  permission: { readonly allowed: boolean; readonly reason: string };
  onApprove: () => void;
}) {
  const { standing } = source;
  const kind = standing.standing;
  const tone = kind === "registered" ? "ok" : kind === "keyless" ? "neutral" : "warn";

  return (
    <Panel data-testid={`registration-${source.source_id}`} data-standing={kind}>
      <PanelHead
        title={source.source_id}
        actions={
          <Chip tone={tone}>
            <span data-testid={`registration-standing-${source.source_id}`}>
              {kind === "keyless" ? "keyless" : kind === "registered" ? `registered by ${standing.operator}` : "pending"}
            </span>
          </Chip>
        }
      />
      <PanelBody>
        <dl className="flex flex-col text-[12px]">
          <Row label="requirement">
            {source.requirement === null ? (
              <Muted>not declared — an unasked question, which the platform refuses as unknown rather than reading as keyless</Muted>
            ) : (
              <>
                <span className="num">{source.requirement}</span>
                <Muted> · {REQUIREMENT_LABEL[source.requirement] ?? "a requirement this console does not name; the platform's word stands"}</Muted>
              </>
            )}
          </Row>

          {kind === "registered" ? (
            <Row label="standing">
              <span data-testid={`registration-record-${source.source_id}`}>
                registered by <span className="num">{standing.operator}</span>, terms read{" "}
                {formatTimestamp(standing.terms_read_at)}
                {/*
                  The variable the *record* names, which is not the same fact
                  as the row's `secret_slot` — a registration mounted from the
                  committed file need not agree with the shipped manifest, and
                  an operator has to be able to see that disagreement. It rides
                  on the operator list, so it is stated only when that list was
                  actually served; the refusal below says why it is otherwise
                  absent, rather than this line quietly saying nothing.
                */}
                {slot !== null && slot.standing.standing === "registered" ? (
                  <>
                    , credential named{" "}
                    <code className="num" data-testid={`registration-record-secret-${source.source_id}`}>
                      {slot.standing.secret}
                    </code>
                  </>
                ) : null}
              </span>
            </Row>
          ) : kind === "pending" ? (
            <Row label="standing">
              <span data-testid={`registration-pending-${source.source_id}`}>
                pending: <span className="num">{standing.who_must_register}</span> must register.{" "}
                <Muted>{standing.reason}</Muted>
              </span>
            </Row>
          ) : (
            <Row label="standing">
              <Muted>no registration is needed; the endpoint is public and the platform reads it as such</Muted>
            </Row>
          )}

          <Row label="terms">
            {source.terms === null ? (
              <Muted>the platform declares no terms reference for this source</Muted>
            ) : (
              <span data-testid={`registration-terms-${source.source_id}`}>
                <TermsReference terms={source.terms} />
                {kind === "pending" ? (
                  <Muted> — read these yourself, in full, before approving; the record names the instant you did</Muted>
                ) : null}
              </span>
            )}
          </Row>

          {slot === null ? (
            <Row label="secret slot">
              <SlotsWithheld access={access} sourceId={source.source_id} />
            </Row>
          ) : (
            <>
              <Row label="secret slot">
                {slot.secret_slot === null ? (
                  // `null` is not the same fact twice. A keyless source reads
                  // no credential at all; a source that needs an account can
                  // still answer `null` here when the manifest the platform
                  // holds names no variable yet (the contract's own
                  // `kalshi-markets` row does), and calling that one keyless
                  // would tell an operator no key is needed for a source the
                  // platform is refusing for want of one. Neither of those is
                  // the third fact — that this console was refused the list —
                  // which is why that one has its own branch above.
                  <Muted testId={`registration-no-slot-${source.source_id}`}>
                    {kind === "keyless"
                      ? "none; a keyless source reads no credential"
                      : "none: the manifest the platform holds for this source names no credential variable, so there is nothing to prefill — the approval needs the variable name this deployment reads the credential under"}
                  </Muted>
                ) : (
                  <code className="num" data-testid={`registration-secret-slot-${source.source_id}`}>
                    {slot.secret_slot}
                  </code>
                )}
              </Row>

              {slot.secret_command === null ? null : (
                <Row label="add the secret">
                  <CopyCommand command={slot.secret_command} sourceId={source.source_id} />
                </Row>
              )}

              {slot.companion_secret_slots.map((companion) => (
                <Row key={companion.variable} label="also reads">
                  <div className="flex min-w-0 flex-col gap-1">
                    <code className="num" data-testid={`registration-companion-slot-${source.source_id}`}>
                      {companion.variable}
                    </code>
                    <CopyCommand
                      command={companion.secret_command}
                      sourceId={`${source.source_id}-${companion.variable}`}
                    />
                  </div>
                </Row>
              ))}
            </>
          )}
        </dl>

        {kind === "pending" ? (
          <div className="mt-3 flex flex-col gap-1">
            <button
              type="button"
              className="btn"
              data-variant="primary"
              disabled={!permission.allowed}
              onClick={onApprove}
              aria-haspopup="dialog"
              data-testid={`registration-approve-${source.source_id}`}
              title={permission.allowed ? undefined : permission.reason}
            >
              Approve registration
            </button>
            {permission.allowed ? null : (
              <span
                className="text-[11px] leading-snug text-[color:var(--color-warn)]"
                data-testid={`registration-approve-reason-${source.source_id}`}
              >
                {permission.reason}
              </span>
            )}
          </div>
        ) : null}
      </PanelBody>
    </Panel>
  );
}

/**
 * What stands where a slot would be when the operator list did not arrive.
 *
 * Never a blank. A blank cell here would be read as the `secret_slot: null`
 * case one branch away — "the manifest names no credential variable" — which
 * is the opposite conclusion: it would tell an operator nothing is needed for
 * a source the platform is refusing until a credential exists. So each of the
 * three non-answers says which it is, and the refusal names the role that may
 * read the route and the two ways an operator actually gets the fact.
 */
function SlotsWithheld({ access, sourceId }: { access: SlotAccess; sourceId: string }) {
  switch (access.status) {
    case "loading":
      return (
        <Muted testId={`registration-slot-loading-${sourceId}`}>
          reading {SLOTS_ROUTE}…
        </Muted>
      );
    case "available":
      // The list was served and this source was not on it. Not a refusal and
      // not an absent slot: the two lists are built from one catalogue in one
      // order, so a row on one and not the other is a disagreement worth
      // saying out loud rather than rendering as "none".
      return (
        <Muted testId={`registration-slot-absent-${sourceId}`}>
          the operator list was served and carries no row for this source, though both lists are
          built from the same catalogue — the platform is disagreeing with itself and this console
          will not pick a side
        </Muted>
      );
    case "refused":
      return (
        <span
          className="flex flex-col gap-1"
          data-testid={`registration-slot-refused-${sourceId}`}
          data-slot-access="refused"
        >
          <span className="text-[11px] leading-snug text-[color:var(--color-warn)]">
            withheld — <span className="num">{SLOTS_ROUTE}</span> answered{" "}
            <span className="num">{access.status_code}</span>. {SLOTS_REFUSAL}
          </span>
          <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">
            The platform said: {access.detail}
          </span>
        </span>
      );
    case "unavailable":
      return (
        <Muted testId={`registration-slot-unavailable-${sourceId}`}>
          {SLOTS_ROUTE} did not answer, so no slot is shown rather than an empty one: {access.detail}
        </Muted>
      );
  }
}

function TermsReference({ terms }: { terms: string }) {
  if (/^https?:\/\//i.test(terms)) {
    return (
      <a href={terms} target="_blank" rel="noreferrer noopener" className="num underline">
        {terms}
      </a>
    );
  }
  return <span className="num">{terms}</span>;
}

/**
 * The command that puts the value in Secret Manager, as the platform wrote it,
 * with a copy control. The value itself is typed by the operator at their
 * own shell; nothing here ever holds it.
 */
function CopyCommand({ command, sourceId }: { command: string; sourceId: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  const copy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(command);
      setState("copied");
    } catch {
      setState("failed");
    }
  }, [command]);

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <code
        className="num block whitespace-pre-wrap break-all border border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-2 py-1 text-[11px]"
        data-testid={`registration-command-${sourceId}`}
      >
        {command}
      </code>
      <div className="flex items-center gap-2">
        <button
          type="button"
          className="btn"
          data-variant="ghost"
          onClick={() => void copy()}
          aria-label={`Copy the add-secret command for ${sourceId}`}
        >
          {state === "copied" ? "copied" : "copy"}
        </button>
        {state === "failed" ? <Muted>the browser refused the clipboard; select the line and copy it</Muted> : null}
      </div>
    </div>
  );
}

/**
 * The confirm step. Two clicks and a statement, because what is asserted is a
 * fact about a person: that they read the terms and registered under the
 * company's identity. What the platform *records* is narrower, and the dialog
 * says which — the console's deployment credential, not the signed-in name.
 * The terms reference and the variable name are prefilled
 * from the platform and editable, since the operator is the one who knows
 * which document they read; both are sent as the operator left them and the
 * platform is the one that refuses a blank or key-shaped value.
 *
 * The variable name can only be prefilled from the operator list, and this
 * console is usually refused it. An empty field is then the honest start:
 * the operator knows which variable this deployment reads the credential
 * under, and a guess made here would be a variable name the console invented
 * and the platform recorded.
 */
function ApproveDialog({
  source,
  slot,
  operatorName,
  onClose,
  onApproved,
}: {
  source: RegistrationSource;
  slot: RegistrationSlotSource | null;
  operatorName: string;
  onClose: () => void;
  onApproved: (approval: Approval) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [terms, setTerms] = useState(source.terms ?? "");
  const [secret, setSecret] = useState(slot?.secret_slot ?? "");
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<ApiOutcome<Approval> | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  const statement = useMemo(
    () =>
      `I have read ${terms.trim().length === 0 ? "the venue's terms" : terms.trim()} and I register this venue under the company's own identity; the platform will not create the account anonymously.`,
    [terms],
  );

  const onSubmit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      setBusy(true);
      setRefusal(null);
      try {
        const response = await registrations.approve(source.source_id, { terms, secret });
        if (response.outcome.kind === "ok") {
          onApproved(response.outcome.data);
        } else {
          setRefusal(response.outcome);
        }
      } finally {
        setBusy(false);
      }
    },
    [source.source_id, terms, secret, onApproved],
  );

  return (
    <dialog
      ref={dialogRef}
      className="w-[min(640px,92vw)] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] p-0 text-[color:var(--color-ink)] backdrop:bg-black/70"
      aria-labelledby="registration-approve-title"
      onClose={onClose}
      onCancel={onClose}
      data-testid="registration-dialog"
    >
      <div className="border-b border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-4 py-2.5">
        <h2 id="registration-approve-title" className="panel-title">
          Approve registration for {source.source_id}
        </h2>
      </div>
      <form onSubmit={onSubmit} className="flex flex-col gap-3 px-4 py-4">
        <p className="text-[12px] leading-relaxed" data-testid="registration-statement">
          {statement}
        </p>
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          Confirming sends <code className="num">POST /api/v1/registrations/{source.source_id}/approve</code>{" "}
          with the terms reference and the variable name below. The platform refuses a blank or
          key-shaped secret with a 400 naming the field, and refuses a credential without the
          operator role with a 403; it creates no account and reads nothing from the venue on the
          strength of this record.
        </p>
        {/*
          The attribution gap, stated where the click is made rather than
          discovered afterwards in the record. This console authenticates to
          the platform with one deployment credential shared by every browser
          session, and the platform has no way to verify a console session's
          identity, so the subject it journals is that credential's — not the
          person at the keyboard. Saying "Confirm as <name>" here named an
          identity nothing downstream records. See the 2026-09-05 amendment to
          ADR 0041.
        */}
        <p
          className="text-[11.5px] leading-relaxed text-[color:var(--color-warn)]"
          data-testid="registration-attribution"
        >
          What the platform records is this console&apos;s own API credential — a deployment
          subject such as <span className="num">operator@env</span>, the same for every person who
          signs in here — and not {operatorName}. That you are the one confirming is known only
          from this console&apos;s sign-in record, which the platform&apos;s event log does not
          hold. Confirm only if you personally read the terms and registered the account.
        </p>
        <div>
          <label className="field-label" htmlFor="registration-terms">
            Terms you read (URL or document name)
          </label>
          <input
            id="registration-terms"
            className="input"
            value={terms}
            onChange={(event) => setTerms(event.target.value)}
            autoComplete="off"
            data-testid="registration-terms"
          />
        </div>
        <div>
          <label className="field-label" htmlFor="registration-secret">
            Variable name the credential lives under — the name, never the value
          </label>
          <input
            id="registration-secret"
            className="input"
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
            autoComplete="off"
            spellCheck={false}
            data-testid="registration-secret"
          />
        </div>
        {refusal === null ? null : (
          <p
            role="alert"
            className="text-[11.5px] leading-relaxed text-[color:var(--color-down)]"
            data-testid="registration-refusal"
          >
            {refusal.kind === "denied"
              ? `The platform refused this console's credential (${refusal.status}): ${refusal.detail}`
              : refusal.kind === "error" && refusal.status === 409
                ? // The kernel accepts an operator identity for fifteen minutes,
                  // as it does for an eligibility decision; the remedy is a
                  // fresh sign-in, not a retry, and the page says so.
                  `your session's credential is older than 15 minutes; sign in again (409: ${refusal.detail})`
                : // Everything else is shown as the platform said it. A 400
                  // from this route names the field it would not read, and
                  // paraphrasing it into "check the fields and try again"
                  // takes the one sentence that says which field and replaces
                  // it with a sentence that says nothing. The route is built
                  // so that this is safe to display: it parses the body by
                  // hand and quotes no value a caller sent, so what comes back
                  // is the platform's own words about its own contract and
                  // never an echo of what was typed into this form.
                  describeOutcome(refusal)}
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
            data-testid="registration-confirm"
          >
            {busy ? "Recording…" : "Confirm approval"}
          </button>
        </div>
      </form>
    </dialog>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline gap-4 border-b border-[color:var(--color-line)] py-1.5 last:border-b-0">
      <dt className="w-[110px] shrink-0 text-[11px] text-[color:var(--color-ink-dim)]">{label}</dt>
      <dd className="min-w-0 flex-1 text-[12px]">{children}</dd>
    </div>
  );
}

function Muted({ children, testId }: { children: ReactNode; testId?: string }) {
  return (
    <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]" data-testid={testId}>
      {children}
    </span>
  );
}
