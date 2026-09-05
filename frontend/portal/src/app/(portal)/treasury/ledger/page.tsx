"use client";

import { Chip, Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useLedgerUsers, type LedgerUser } from "@/lib/hooks/useTreasury";
import { CapabilityChip, Muted, TreasuryHeader, WithdrawalChip } from "../_shared";

/**
 * The per-user, per-strategy ledger (blueprint §43.3, §43.4), read-only.
 *
 * `GET /ledger/users` answers every user the ledger holds a mandate for, the
 * terms of that mandate, one balance row per `(strategy, currency)` book, and
 * the entitlements it evaluated for the viewer role at request time. This
 * page renders those fields and nothing derived from them.
 *
 * Two things are deliberate about the balances table. `available` is the
 * platform's own `settled - reserved`; the page shows the figure the route
 * answered rather than subtracting for itself. And expected inflows are a
 * separate column with the platform's separate total: a deposit the user
 * says is on its way is a claim the ledger has not yet seen,
 * `CashBalance::available` excludes it by construction, and a page that
 * folded it into a headline number would be sizing the reader's expectations
 * against money that may never arrive.
 *
 * The withdrawal entitlement is rendered as refused with the platform's
 * reason on every row, because that is the only value the platform's type can
 * hold (ADR 0021, ADR 0023).
 */
export default function LedgerPage() {
  const ledger = useLedgerUsers();

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Ledger"
        reads="GET /ledger/users"
        posture={ledger.data?.posture ?? null}
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
                    {data.users.map((user) => (
                      <UserCard key={user.user_id} user={user} />
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

function UserCard({ user }: { user: LedgerUser }) {
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
