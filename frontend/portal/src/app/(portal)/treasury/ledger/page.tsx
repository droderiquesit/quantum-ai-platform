"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Chip, Freshness, KeyValue, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { SystemStatus } from "@/lib/api/types";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import {
  eligibilityPermission,
  useLedgerUsers,
  useSessionIdentity,
  type EligibilityPermission,
  type LedgerUser,
  type SessionIdentity,
} from "@/lib/hooks/useTreasury";
import { CapabilityChip, Muted, WithdrawalChip } from "../_shared";
import { EligibilityPanel } from "./EligibilityPanel";

/**
 * The per-user, per-strategy ledger (blueprint §43.3, §43.4).
 *
 * `GET /ledger/users` answers every user the ledger holds a mandate for, the
 * terms of that mandate, the ledger's own eligibility verdict on them, one
 * balance row per `(strategy, currency)` book, and the entitlements it
 * evaluated for the viewer role at request time. This page renders those
 * fields and nothing derived from them.
 *
 * Three things are deliberate about it.
 *
 * `available` is the platform's own `settled - reserved`; the page shows the
 * figure the route answered rather than subtracting for itself. And expected
 * inflows are a separate column with the platform's separate total: a deposit
 * the user says is on its way is a claim the ledger has not yet seen,
 * `CashBalance::available` excludes it by construction, and a page that folded
 * it into a headline number would be sizing the reader's expectations against
 * money that may never arrive.
 *
 * The withdrawal entitlement is rendered as refused with the platform's reason
 * on every row, because that is the only value the platform's type can hold
 * (ADR 0021, ADR 0023).
 *
 * And the eligibility verdict is rendered on every row — eligible with the
 * terms an operator wrote, or the ledger's own refusal token with its
 * sentence — because a page listing a user with balances and no way to tell
 * whether the next funding would be refused, and why, is the page the desk
 * had before. Beside it sits the one control this section holds: an
 * operator's decision about that user. It records a finding about a person and
 * moves no capital, names no instrument, and carries no field about capital
 * leaving the platform.
 */
export default function LedgerPage() {
  const ledger = useLedgerUsers();
  const identity = useSessionIdentity();
  const permission = eligibilityPermission(identity);
  const operatorName = identity.status === "authenticated" ? identity.name : null;

  // The row the platform answered to a decision, shown in place of the one the
  // list carried until the next GET lands. The platform's own list wins on
  // every refresh: a page that kept its memory of a click over the platform's
  // record would be the page showing "eligible" the day the record was lost.
  const [decided, setDecided] = useState<ReadonlyMap<string, LedgerUser>>(new Map());
  const lastReceived = useRef<number | null>(ledger.receivedAt);
  useEffect(() => {
    if (ledger.receivedAt !== lastReceived.current) {
      lastReceived.current = ledger.receivedAt;
      setDecided(new Map());
    }
  }, [ledger.receivedAt]);

  const refresh = ledger.refresh;
  const onDecided = useCallback(
    (row: LedgerUser) => {
      setDecided((current) => new Map(current).set(row.user_id, row));
      refresh();
    },
    [refresh],
  );

  return (
    <div className="flex flex-col gap-3 p-3">
      <LedgerHeader
        posture={ledger.data?.posture ?? null}
        identity={identity}
        meta={<Freshness resource={ledger} name="ledger" />}
      />

      <Panel>
        <PanelHead title="Users and mandates" />
        <PanelBody>
          <ResourceView resource={ledger} loadingRows={3}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Users with a mandate"
                    value={<span data-testid="ledger-user-count">{formatCount(data.users.length)}</span>}
                    note="GET /ledger/users: users"
                  />
                  <Kpi
                    label="Balance rows"
                    value={formatCount(data.users.reduce((sum, user) => sum + user.balances.length, 0))}
                    note="one per (user, strategy, currency) the ledger has opened"
                  />
                  <Kpi
                    label="Fills journalled"
                    value={formatCount(data.fills_journalled)}
                    note="attributed fills booked across every user"
                    tone="info"
                  />
                  <Kpi
                    label="Products"
                    value={formatCount(data.products.length)}
                    note={
                      data.products.length === 0
                        ? "no strategy family registered; entitlements evaluate against none"
                        : data.products.join(", ")
                    }
                  />
                  <Kpi
                    label="Entitlements evaluated as"
                    value={<span className="text-[15px]">{data.evaluated_as_role}</span>}
                    note="this surface is the viewer's; it never evaluates as an investor or the desk"
                  />
                </KpiRow>
                {data.users.length === 0 ? (
                  <div className="mt-3">
                    <EmptyBlock headline="The ledger holds no mandate.">
                      <p>
                        No user has been enrolled in this process. A mandate is the object the
                        attribution chain terminates in before the user, so until one exists there
                        is no book to show — this is an observed empty ledger, not an unread one.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <div className="mt-3 flex flex-col gap-3">
                    {data.users.map((listed) => (
                      <UserCard
                        key={listed.user_id}
                        user={decided.get(listed.user_id) ?? listed}
                        operatorName={operatorName}
                        permission={permission}
                        onDecided={onDecided}
                      />
                    ))}
                  </div>
                )}
                <p className="mt-2">
                  <Muted>served {formatTimestamp(data.served_at)} · posture reported by the body: {data.posture}</Muted>
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * The declaration, on the page and not only in the chrome.
 *
 * The treasury section shares `TreasuryHeader`, whose sentence states that the
 * page holds no control and that the gateway declares no write it could call.
 * That is true of the wallet, the corridors and the transfer gate and it is no
 * longer true here, so this page carries its own — because a page that grew a
 * control while still displaying the sentence "there is no control here" would
 * be lying in exactly the place an operator goes to check.
 *
 * Everything else is the shared header's, deliberately: the body's own
 * `posture` literal rendered as it came, so a body that ever said something
 * else would be shown saying it; and the platform's live capability from
 * `GET /system/status`, read the same way the other treasury pages read it.
 */
function LedgerHeader({
  posture,
  identity,
  meta,
}: {
  posture: string | null;
  identity: SessionIdentity;
  meta: ReactNode;
}) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "treasury-status-ledger",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="treasury-header">
      <PanelHead
        title="Ledger"
        meta={meta}
        actions={
          <>
            {posture === null ? null : (
              <Chip tone={posture === "PAPER TRADING" ? "ok" : "bad"} title="GET /ledger/users: posture">
                <span data-testid="treasury-body-posture">{posture}</span>
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
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]" data-testid="treasury-declaration">
          <span className="chip mr-2" data-tone="ok" data-testid="treasury-paper-label">
            PAPER TRADING
          </span>
          Nothing on this page can move capital. It reads{" "}
          <span className="num">GET /ledger/users</span> and renders what the platform answered. Its
          one control records an operator&rsquo;s eligibility decision about a user — who was
          verified, when, where, until when, and against which document — and that record moves no
          money: it names no instrument, side, quantity or price, and it carries no field about
          capital leaving the platform. There is nothing here that proposes, signs or transfers.
          ADR 0021 refuses the half of the treasury by which capital leaves and ADR 0023 keeps that
          in force.
        </p>
        <p className="mt-2 text-[11px] leading-snug text-[color:var(--color-ink-faint)]" data-testid="ledger-session">
          {identity.status === "loading"
            ? "reading who is signed in…"
            : identity.status === "unauthenticated"
              ? "no one is signed in to this console; an eligibility decision needs a named operator"
              : `signed in as ${identity.name} (${identity.email}), roles: ${identity.roles.length === 0 ? "none" : identity.roles.join(", ")}`}
        </p>
      </PanelBody>
    </Panel>
  );
}

function UserCard({
  user,
  operatorName,
  permission,
  onDecided,
}: {
  user: LedgerUser;
  operatorName: string | null;
  permission: EligibilityPermission;
  onDecided: (row: LedgerUser) => void;
}) {
  const mandate = user.mandate;
  return (
    <section
      className="flex flex-col gap-3 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2"
      data-testid="ledger-user"
      aria-label={`ledger for ${user.user_id}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="num text-[14px] font-semibold">{user.user_id}</span>
        <Chip tone="info">{mandate.jurisdiction}</Chip>
        <Chip>{mandate.currency}</Chip>
        <Chip>
          {mandate.permitted_families.any
            ? "any family"
            : `${formatCount(mandate.permitted_families.families.length)} permitted family(ies)`}
        </Chip>
      </div>

      <div className="grid gap-4" style={{ gridTemplateColumns: "minmax(220px, 1fr) 2fr" }}>
        <div>
          <span className="eyebrow">mandate</span>
          <dl className="mt-1">
            <KeyValue label="Capital under management">
              {formatDecimal(mandate.capital)} {mandate.currency}
            </KeyValue>
            <KeyValue label="Liquidity floor">
              {formatDecimal(mandate.liquidity_floor)} {mandate.currency}
            </KeyValue>
            <KeyValue label="Investable (platform's figure)">
              {formatDecimal(mandate.investable)} {mandate.currency}
            </KeyValue>
            <KeyValue label="Risk tolerance">{formatDecimal(mandate.risk_tolerance)}</KeyValue>
            <KeyValue label="Exploration share">{formatDecimal(mandate.exploration_share)}</KeyValue>
            <KeyValue label="Permitted families" mono={false}>
              {mandate.permitted_families.any ? "any" : mandate.permitted_families.families.join(", ")}
            </KeyValue>
          </dl>
        </div>

        <div>
          <span className="eyebrow">per-strategy balances</span>
          {user.balances.length === 0 ? (
            <p className="mt-1">
              <Muted>
                no book has been opened for this user; a row appears once a fill has been attributed
                to them
              </Muted>
            </p>
          ) : (
            <TableWell maxHeight="320px" label={`balances for ${user.user_id}`}>
              <table className="dt" data-testid="ledger-balances">
                <thead>
                  <tr>
                    <th scope="col">Strategy</th>
                    <th scope="col">Currency</th>
                    <th scope="col" className="n">
                      Settled
                    </th>
                    <th scope="col" className="n">
                      Reserved
                    </th>
                    <th scope="col" className="n">
                      Available
                    </th>
                    <th scope="col" className="n">
                      Expected inflows (not available)
                    </th>
                    <th scope="col" className="n">
                      Entries
                    </th>
                    <th scope="col">Last entry</th>
                  </tr>
                </thead>
                <tbody>
                  {user.balances.map((balance) => (
                    <tr key={`${balance.strategy}:${balance.currency}`} data-testid="ledger-balance-row">
                      <td className="num">{balance.strategy}</td>
                      <td className="num">{balance.currency}</td>
                      <td className="n">{formatDecimal(balance.settled)}</td>
                      <td className="n">{formatDecimal(balance.reserved)}</td>
                      <td className="n" data-testid="ledger-available">
                        {formatDecimal(balance.available)}
                      </td>
                      <td className="n" data-testid="ledger-expected">
                        {formatDecimal(balance.expected_inflows_total)}
                        {balance.expected_inflows.length > 0 ? (
                          <span className="block">
                            <Muted>
                              {balance.expected_inflows
                                .map(
                                  (inflow) =>
                                    `${inflow.reference}: ${formatDecimal(inflow.amount)} declared ${formatTimestamp(inflow.declared_at)}`,
                                )
                                .join(" · ")}
                            </Muted>
                          </span>
                        ) : null}
                      </td>
                      <td className="n">{formatCount(balance.entries)}</td>
                      <td className="num">{formatTimestamp(balance.last_entry_at)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableWell>
          )}
          <p className="mt-1">
            <Muted>
              Available is the platform&rsquo;s settled less reserved. Expected inflows are declared,
              not posted, and are shown beside the balance rather than in it: the ledger excludes
              them from available until it has seen the money, and so does this page.
            </Muted>
          </p>
        </div>
      </div>

      <EligibilityVerdict user={user} />

      <EligibilityPanel
        user={user}
        operatorName={operatorName}
        permission={permission}
        onDecided={onDecided}
      />

      <div>
        <span className="eyebrow">entitlements, as last evaluated</span>
        {user.entitlements.length === 0 ? (
          <p className="mt-1" data-testid="ledger-entitlements-note">
            <Muted>{user.entitlements_note ?? "no entitlement has been evaluated for this user"}</Muted>
          </p>
        ) : (
          <ul className="mt-1 flex flex-col gap-2">
            {user.entitlements.map((entitlement) => (
              <li
                key={`${entitlement.family}:${entitlement.role}:${entitlement.evaluated_at}`}
                className="grid gap-3 border-t border-[color:var(--color-line)] pt-2"
                style={{ gridTemplateColumns: "minmax(160px, 0.8fr) repeat(3, 1fr)" }}
                data-testid="ledger-entitlement"
              >
                <div className="flex flex-col gap-0.5">
                  <span className="num text-[12px]">{entitlement.family}</span>
                  <Muted>
                    {entitlement.role} · evaluated {formatTimestamp(entitlement.evaluated_at)}
                  </Muted>
                </div>
                <CapabilityChip label="can view" capability={entitlement.can_view} />
                <CapabilityChip label="can invest" capability={entitlement.can_invest} />
                <WithdrawalChip entitlement={entitlement.can_withdraw} />
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}

/**
 * The ledger's verdict on this user, as it answered it.
 *
 * Three states and no fourth. Eligible, with the terms an operator wrote;
 * refused, with the ledger's stable token and its own sentence, rendered
 * verbatim because the sentence names what to do; or a row that carried no
 * verdict at all, which is stated in those words. The third is not read as
 * eligible and not read as refused: a console that guessed either way about a
 * field the platform did not send would be inventing the answer to the one
 * question this block exists to ask.
 */
function EligibilityVerdict({ user }: { user: LedgerUser }) {
  const verdict = user.eligibility;
  const id = user.user_id;

  if (verdict === undefined) {
    return (
      <div data-testid={`eligibility-${id}`} data-eligible="unknown">
        <span className="eyebrow">eligibility, as the ledger decided it at request time</span>
        <p className="mt-1 flex flex-wrap items-center gap-2">
          <Chip tone="warn">
            <span data-testid={`eligibility-verdict-${id}`}>no verdict answered</span>
          </Chip>
          <Muted>
            this row carried no eligibility field. That is not an eligible user and not a refused
            one; it is a process serving a shape from before the verdict existed, and the answer to
            &ldquo;may capital be put to work for this person&rdquo; is unknown until it does.
          </Muted>
        </p>
      </div>
    );
  }

  return (
    <div data-testid={`eligibility-${id}`} data-eligible={verdict.eligible ? "true" : "false"}>
      <span className="eyebrow">eligibility, as the ledger decided it at request time</span>
      <div className="mt-1 flex flex-wrap items-center gap-2">
        <Chip tone={verdict.eligible ? "ok" : "warn"}>
          <span data-testid={`eligibility-verdict-${id}`}>
            {verdict.eligible ? "eligible" : (verdict.refused ?? "not eligible")}
          </span>
        </Chip>
        {verdict.eligible ? (
          <span className="text-[11.5px]" data-testid={`eligibility-terms-${id}`}>
            <Muted>
              verified {formatTimestamp(verdict.verified_at)} in{" "}
              {verdict.jurisdiction ?? "an unstated jurisdiction"} ·{" "}
              {verdict.can_invest === true
                ? "may have capital put to work"
                : "cleared to view and not to invest"}{" "}
              · expires {formatTimestamp(verdict.expires_at)}
            </Muted>
          </span>
        ) : null}
      </div>
      {verdict.eligible ? null : (
        <p className="mt-1 text-[11.5px] leading-relaxed" data-testid={`eligibility-reason-${id}`}>
          <Muted>{verdict.reason ?? "the platform gave no reason."}</Muted>
        </p>
      )}
    </div>
  );
}
