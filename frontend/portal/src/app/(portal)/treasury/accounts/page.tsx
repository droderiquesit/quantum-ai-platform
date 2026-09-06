"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Suspense } from "react";
import { Chip, Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import {
  EmptyBlock,
  LoadingBlock,
  MissingEndpointBlock,
  ResourceView,
  StateBlock,
} from "@/components/data/States";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import {
  useLedgerUsers,
  useSessionIdentity,
  type LedgerUser,
  type LedgerUsers,
} from "@/lib/hooks/useTreasury";
import { CapabilityChip, Muted, TreasuryHeader, WithdrawalChip } from "../_shared";

/**
 * One account, whole (blueprint §43.3): the mandate it holds, the ledger's own
 * verdict on whether capital may be put to work for it, every book opened for
 * it, and what it may do with each product.
 *
 * The ledger page is the desk's list — every account at once, for comparison.
 * This is the account itself, for the question an operator asks about one
 * person: what did we agree to, is that agreement currently good, where is
 * their money, and what may they do next. Same route, same fields, no figure
 * derived: `GET /ledger/users` answers all of it and this page renders the one
 * row the reader asked for.
 *
 * **Whose account this is, said out loud.** The platform serves no per-user
 * route and no session-bound account view: `/ledger/users` requires the analyst
 * role, answers every account, and evaluates every entitlement as the ledger's
 * viewer role against a ledger user id. This console reaches it with one
 * deployment credential — a subject such as `operator@env`, the same for every
 * person who signs in here — so the account on screen is the one an operator
 * selected, never the signed-in person's own. A page that greeted the reader
 * with "your account" would be naming a binding that does not exist anywhere
 * below it. The two routes that would make it exist are named on the page
 * itself, in the "Whose account this is" panel, out of the same
 * `NOT_YET_SERVED` table every other absent endpoint on this console renders
 * from — because until they were, the reason this page shows an operator's
 * pick lived only in this comment and in a handoff document, and neither is
 * legible from a screen.
 *
 * **The states are four and they do not look alike.** A platform that could not
 * be reached, a credential the route refused, a ledger holding no account at
 * all, and an account id this ledger does not hold each render as their own
 * block saying which. The last matters because it is what a bookmark becomes
 * after a mandate is retired, and reading it as "no accounts" would tell an
 * operator the ledger is empty when it is not.
 *
 * Nothing here moves capital. There is no control on this page at all: the one
 * write the treasury surface has — an operator's eligibility decision — lives
 * on the ledger page beside the list it changes, and the withdrawal that would
 * be the other one does not exist and may not (ADR 0021, ADR 0023).
 */
export default function AccountPage() {
  return (
    <Suspense fallback={<LoadingBlock rows={4} label="reading which account was asked for" />}>
      <Account />
    </Suspense>
  );
}

function Account() {
  const ledger = useLedgerUsers();
  // The account asked for, as a query parameter so a row is linkable. Absent
  // means "the first the ledger lists", which is a default and is labelled as
  // one — never a claim that this is the reader's account.
  const asked = useSearchParams().get("user");

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Account"
        reads="GET /ledger/users"
        posture={ledger.data?.posture ?? null}
        meta={<Freshness resource={ledger} name="account" />}
      />

      <Panel>
        <PanelHead title="One account, as the ledger holds it" />
        <PanelBody>
          <ResourceView resource={ledger} loadingRows={4}>
            {(data) => <Selected data={data} asked={asked} />}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Whose account this is" />
        <PanelBody>
          <WhoseAccount />
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * The binding that does not exist, named where a reader would otherwise assume
 * it does.
 *
 * The page's own doc comment has said this since it landed, and a source
 * comment is not something an operator looking at the screen can read. So the
 * two routes that would close it are in `NOT_YET_SERVED` — the same table
 * every other absent endpoint on this console is rendered from — and this
 * panel renders them beside who is actually signed in.
 *
 * The attribution wording is the one agreed for the venue-registration
 * dialog and it is deliberately the same: what the platform records is this
 * console's own deployment credential, a subject such as `operator@env`, and
 * naming the signed-in person as the subject of a platform-recorded fact
 * names an identity nothing downstream holds. Here the fact is which account
 * the reader is looking at, which is a read rather than a write, and the gap
 * is the same one.
 */
function WhoseAccount() {
  const identity = useSessionIdentity();

  return (
    <div className="flex flex-col gap-3">
      <p
        className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
        data-testid="account-binding"
      >
        The account above is the one an operator selected. It is not resolved from whoever is signed
        in to this console, because nothing below this page can resolve that: every browser session
        reaches the platform through one deployment credential, so the subject the platform sees is
        that credential&rsquo;s — something of the form{" "}
        <span className="num">operator@env</span>, the same for every person who signs in here — and
        it is not a ledger account. Which human is reading is known only from this console&rsquo;s
        own sign-in record, which the platform&rsquo;s event log does not hold.
      </p>

      <p className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]" data-testid="account-session">
        {identity.status === "loading"
          ? "reading who is signed in…"
          : identity.status === "unauthenticated"
            ? "no one is signed in to this console; the account above would be the same one either way, because the platform is not asked who is reading"
            : `signed in as ${identity.name} (${identity.email}), roles: ${
                identity.roles.length === 0 ? "none" : identity.roles.join(", ")
              } — a console identity, not a ledger account`}
      </p>

      <MissingEndpointBlock endpoint={NOT_YET_SERVED["accountForSession"]!} />
      <MissingEndpointBlock endpoint={NOT_YET_SERVED["accountByUser"]!} />

      <p className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">
        Until the first of those exists this console must not say &ldquo;your account&rdquo;, and it
        does not. Nothing on this page is filled in to stand in for either route.
      </p>
    </div>
  );
}

function Selected({ data, asked }: { data: LedgerUsers; asked: string | null }) {
  if (data.users.length === 0) {
    return (
      <div data-testid="account-none">
        <EmptyBlock headline="The ledger holds no account.">
          <p>
            <span className="num">GET /ledger/users</span> answered and its{" "}
            <span className="num">users</span> list is empty: no mandate has been enrolled in this
            process, so there is no account to show. A mandate is the object the attribution chain
            terminates in before the user, so until one exists there is no book either. This is an
            observed empty ledger — a read that succeeded — and not a platform this console failed
            to reach, which says so in its own words and its own colour.
          </p>
        </EmptyBlock>
      </div>
    );
  }

  const selected = asked === null ? data.users[0] : data.users.find((u) => u.user_id === asked);

  return (
    <>
      <AccountPicker users={data.users} selected={selected?.user_id ?? null} defaulted={asked === null} />
      {selected === undefined ? (
        <div className="mt-3" data-testid="account-unknown">
          <StateBlock
            tone="warn"
            label="no such account"
            headline={`The ledger holds no account "${asked}".`}
          >
            <p>
              The route answered {formatCount(data.users.length)} account
              {data.users.length === 1 ? "" : "s"} and this is not one of them. That is different
              from a ledger with nothing in it, and different again from a platform that could not
              be reached: the accounts it does hold are listed above, and one of them is a click
              away. A mandate that has been retired leaves a link like this one behind.
            </p>
          </StateBlock>
        </div>
      ) : (
        <AccountDetail user={selected} data={data} />
      )}
    </>
  );
}

function AccountPicker({
  users,
  selected,
  defaulted,
}: {
  users: readonly LedgerUser[];
  selected: string | null;
  defaulted: boolean;
}) {
  return (
    <div className="mt-1 flex flex-col gap-1" data-testid="account-picker">
      <span className="eyebrow">accounts the ledger holds</span>
      <div className="flex flex-wrap items-center gap-1.5">
        {users.map((user) => (
          <Link
            key={user.user_id}
            href={`/treasury/accounts?user=${encodeURIComponent(user.user_id)}`}
            className="chip"
            data-tone={user.user_id === selected ? "ok" : undefined}
            data-testid="account-choice"
            data-user={user.user_id}
            aria-current={user.user_id === selected ? "page" : undefined}
          >
            {user.user_id}
          </Link>
        ))}
      </div>
      {defaulted && selected !== null ? (
        <p data-testid="account-defaulted">
          <Muted>
            No account was asked for, so this is the first the ledger listed — a default this page
            chose, not the account of whoever is signed in. This console holds one deployment
            credential (a subject such as <span className="num">operator@env</span>, the same for
            every person who signs in here) and the platform binds no console session to a ledger
            account.
          </Muted>
        </p>
      ) : null}
    </div>
  );
}

function AccountDetail({ user, data }: { user: LedgerUser; data: LedgerUsers }) {
  const mandate = user.mandate;
  const booked = user.balances.reduce((sum, balance) => sum + balance.entries, 0);

  return (
    <section
      className="mt-3 flex flex-col gap-3"
      data-testid="account-detail"
      data-user={user.user_id}
      aria-label={`account ${user.user_id}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="num text-[15px] font-semibold" data-testid="account-id">
          {user.user_id}
        </span>
        <Chip tone="info">{mandate.jurisdiction}</Chip>
        <Chip>{mandate.currency}</Chip>
        <Chip>
          {mandate.permitted_families.any
            ? "any family"
            : `${formatCount(mandate.permitted_families.families.length)} permitted family(ies)`}
        </Chip>
      </div>

      <KpiRow>
        <Kpi
          label="Capital under management"
          value={<span data-testid="account-capital">{formatDecimal(mandate.capital)}</span>}
          unit={mandate.currency}
          note="the mandate's own figure"
        />
        <Kpi
          label="Investable"
          value={<span data-testid="account-investable">{formatDecimal(mandate.investable)}</span>}
          unit={mandate.currency}
          note="capital less the liquidity floor, as the platform computed it"
        />
        <Kpi
          label="Books opened"
          value={<span data-testid="account-book-count">{formatCount(user.balances.length)}</span>}
          note="one per (strategy, currency) the ledger has opened for this account"
        />
        <Kpi
          label="Fills booked here"
          value={<span data-testid="account-entries">{formatCount(booked)}</span>}
          note="attributed fills across this account's books; distinguishes none booked from a zero balance"
          tone="info"
        />
      </KpiRow>

      <AccountEligibility user={user} />

      <div>
        <span className="eyebrow">mandate, as agreed</span>
        <dl className="mt-1" style={{ maxWidth: "520px" }}>
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
          <KeyValue label="Jurisdiction">{mandate.jurisdiction}</KeyValue>
          <KeyValue label="Permitted families" mono={false}>
            {mandate.permitted_families.any
              ? "any"
              : mandate.permitted_families.families.join(", ")}
          </KeyValue>
        </dl>
      </div>

      <div>
        <span className="eyebrow">books</span>
        {user.balances.length === 0 ? (
          <div className="mt-1" data-testid="account-no-books">
            <EmptyBlock headline="No book has been opened for this account.">
              <p>
                The ledger opens a book when this account&rsquo;s mandate funds a strategy, and adds
                to it when a fill the centre settled is attributed here. Neither has happened, so
                there is no balance — an account with no book, not a book at zero. Nothing is
                shown in its place.
              </p>
            </EmptyBlock>
          </div>
        ) : (
          <TableWell maxHeight="320px" label={`books for ${user.user_id}`}>
            <table className="dt" data-testid="account-balances">
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
                  <tr key={`${balance.strategy}:${balance.currency}`} data-testid="account-balance-row">
                    <td className="num">{balance.strategy}</td>
                    <td className="num">{balance.currency}</td>
                    <td className="n">{formatDecimal(balance.settled)}</td>
                    <td className="n">{formatDecimal(balance.reserved)}</td>
                    <td className="n" data-testid="account-available">
                      {formatDecimal(balance.available)}
                    </td>
                    <td className="n" data-testid="account-expected">
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
            Available is the platform&rsquo;s settled less reserved. Expected inflows are declared
            and not posted: the ledger keeps them out of available until it has seen the money, and
            so does this page.
          </Muted>
        </p>
      </div>

      <div>
        <span className="eyebrow">
          entitlements, as the platform evaluated them for the {data.evaluated_as_role} role
        </span>
        {user.entitlements.length === 0 ? (
          <p className="mt-1" data-testid="account-entitlements-note">
            <Muted>
              {user.entitlements_note ??
                "the platform answered no entitlement for this account and gave no reason"}
            </Muted>
          </p>
        ) : (
          <ul className="mt-1 flex flex-col gap-2">
            {user.entitlements.map((entitlement) => (
              <li
                key={`${entitlement.family}:${entitlement.role}:${entitlement.evaluated_at}`}
                className="grid gap-3 border-t border-[color:var(--color-line)] pt-2"
                style={{ gridTemplateColumns: "minmax(160px, 0.8fr) repeat(3, 1fr)" }}
                data-testid="account-entitlement"
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
        <p className="mt-1">
          <Muted>
            Evaluated as the ledger&rsquo;s {data.evaluated_as_role} role against this user id, not
            as the person reading. Withdrawal is refused on every row and no record of one exists:
            the eligibility this account is funded against carries no withdrawal field at all — not
            a field that is always false, but a field that is not there (ADR 0021, ADR 0023).
          </Muted>
        </p>
      </div>

      <p>
        <Muted>
          served {formatTimestamp(data.served_at)} · posture reported by the body: {data.posture}
        </Muted>
      </p>
    </section>
  );
}

/**
 * The ledger's verdict on this account, in its own words.
 *
 * Three arms and no fourth. Eligible, with the terms an operator wrote;
 * refused, with the ledger's stable token and the sentence naming what to do;
 * or a row that carried no verdict, stated as unknown. A console that read an
 * absent verdict as either answer would be inventing the answer to the one
 * question this block exists to ask.
 */
function AccountEligibility({ user }: { user: LedgerUser }) {
  const verdict = user.eligibility;

  if (verdict === undefined) {
    return (
      <div data-testid="account-eligibility" data-eligible="unknown">
        <span className="eyebrow">eligibility, as the ledger decided it at request time</span>
        <p className="mt-1 flex flex-wrap items-center gap-2">
          <Chip tone="warn">
            <span data-testid="account-eligibility-verdict">no verdict answered</span>
          </Chip>
          <Muted>
            this row carried no eligibility field. That is not an eligible account and not a refused
            one; it is a process serving a shape from before the verdict existed, and whether
            capital may be put to work here is unknown until it does.
          </Muted>
        </p>
      </div>
    );
  }

  return (
    <div data-testid="account-eligibility" data-eligible={verdict.eligible ? "true" : "false"}>
      <span className="eyebrow">eligibility, as the ledger decided it at request time</span>
      <div className="mt-1 flex flex-wrap items-center gap-2">
        <Chip tone={verdict.eligible ? "ok" : "warn"}>
          <span data-testid="account-eligibility-verdict">
            {verdict.eligible ? "eligible" : (verdict.refused ?? "not eligible")}
          </span>
        </Chip>
        {verdict.eligible ? (
          <span className="text-[11.5px]" data-testid="account-eligibility-terms">
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
        <p className="mt-1 text-[11.5px] leading-relaxed" data-testid="account-eligibility-reason">
          <Muted>{verdict.reason ?? "the platform gave no reason."}</Muted>
        </p>
      )}
    </div>
  );
}
