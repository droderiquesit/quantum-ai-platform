"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useLedgerUsers, type LedgerUsers } from "@/lib/hooks/useTreasury";
import { Muted, TreasuryHeader } from "../_shared";

/**
 * The mandate register (blueprint §43.3): every agreement this deployment
 * manages capital under, side by side.
 *
 * The ledger page carries a mandate too, inside a card per user, beside that
 * user's balances, eligibility verdict, entitlements and the one operator
 * control the treasury surface has. That answers "what is true of Alice". It
 * cannot answer the question a desk asks about the agreements themselves —
 * which jurisdictions are we managing capital in, which mandates keep capital
 * liquid, which set anything aside for exploration, which families may each
 * reach — because answering it means reading every mandate at once and a card
 * per user does not fit on a screen once there are more than two.
 *
 * Every figure here is a field `GET /ledger/users` answered, rendered as its
 * own text. Nothing is summed: a money figure is the platform's `Decimal`
 * serialised exactly, and a page that added two of them in JavaScript would be
 * reporting a number the platform never computed, in a type that cannot hold
 * it. `investable` is on the row because the *platform* computed it.
 *
 * **What this register cannot say, it says it cannot say.** The platform holds
 * three facts about a mandate that `GET /ledger/users` does not project, and
 * the second panel names each with the accessor that holds it, because the
 * difference between "the platform does not know" and "this route does not
 * carry it" is the difference between a gap in the system and a gap in a view.
 * See `MandateRegister` below.
 *
 * **There is no control here and there could not be.** A mandate is enrolled
 * from the deployment's committed configuration — `UserMandate` in the
 * kernel's config, whose own comment says a mandate invented at runtime would
 * be "capital the platform promised to somebody nobody named" — and no route
 * in `qip-api` creates, amends or retires one. A page offering an "edit
 * mandate" control would be implying a path that does not exist, which is the
 * same failure as implying a live order path, one floor down.
 */
export default function MandatesPage() {
  const ledger = useLedgerUsers();

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Mandates"
        reads="GET /ledger/users"
        posture={ledger.data?.posture ?? null}
        meta={<Freshness resource={ledger} name="mandates" />}
      />

      <Panel>
        <PanelHead title="The mandate register" />
        <PanelBody>
          <ResourceView resource={ledger} loadingRows={4}>
            {(data) => <Register data={data} />}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What the register does not carry" />
        <PanelBody>
          <NotProjected />
        </PanelBody>
      </Panel>
    </div>
  );
}

function Register({ data }: { data: LedgerUsers }) {
  if (data.users.length === 0) {
    return (
      <div data-testid="mandate-none">
        <EmptyBlock headline="The ledger holds no mandate.">
          <p>
            <span className="num">GET /ledger/users</span> answered and its{" "}
            <span className="num">users</span> list is empty. No mandate has been enrolled in this
            process, so there is no agreement to show and no book opened under one. This is an
            observed empty register — a read that succeeded and found nothing — and not a platform
            this console failed to reach, which is a different fact with a different remedy and says
            so in its own words and its own colour.
          </p>
        </EmptyBlock>
      </div>
    );
  }

  // Distinct jurisdictions, counted rather than summed: a count of strings the
  // platform sent is a fact about the answer, where an arithmetic on the money
  // strings beside them would be a figure nobody computed.
  const jurisdictions = [...new Set(data.users.map((user) => user.mandate.jurisdiction))].sort();

  return (
    <>
      <KpiRow>
        <Kpi
          label="Mandates held"
          value={<span data-testid="mandate-count">{formatCount(data.users.length)}</span>}
          note="GET /ledger/users: one mandate per user in the registry, the desk's included"
        />
        <Kpi
          label="Jurisdictions"
          value={<span data-testid="mandate-jurisdictions">{formatCount(jurisdictions.length)}</span>}
          note={jurisdictions.join(", ")}
        />
        <Kpi
          label="Products registered"
          value={<span data-testid="mandate-products">{formatCount(data.products.length)}</span>}
          note={
            data.products.length === 0
              ? "the central factory has registered no strategy family; a permitted family names one that does not exist here"
              : data.products.join(", ")
          }
          tone="info"
        />
      </KpiRow>

      <div className="mt-3">
        <TableWell maxHeight="460px" label="the mandate register">
          <table className="dt" data-testid="mandate-table">
            <thead>
              <tr>
                <th scope="col">User</th>
                <th scope="col">Jurisdiction</th>
                <th scope="col">Currency</th>
                <th scope="col" className="n">
                  Capital under management
                </th>
                <th scope="col" className="n">
                  Liquidity floor
                </th>
                <th scope="col" className="n">
                  Investable (platform&rsquo;s figure)
                </th>
                <th scope="col" className="n">
                  Risk tolerance
                </th>
                <th scope="col" className="n">
                  Exploration share
                </th>
                <th scope="col">Permitted families</th>
              </tr>
            </thead>
            <tbody>
              {data.users.map((user) => (
                <tr key={user.user_id} data-testid="mandate-row" data-user={user.user_id}>
                  <td className="num">{user.user_id}</td>
                  <td className="num" data-testid="mandate-jurisdiction">
                    {user.mandate.jurisdiction}
                  </td>
                  <td className="num">{user.mandate.currency}</td>
                  <td className="n" data-testid="mandate-capital">
                    {formatDecimal(user.mandate.capital)}
                  </td>
                  <td className="n" data-testid="mandate-floor">
                    {formatDecimal(user.mandate.liquidity_floor)}
                  </td>
                  <td className="n" data-testid="mandate-investable">
                    {formatDecimal(user.mandate.investable)}
                  </td>
                  <td className="n">{formatDecimal(user.mandate.risk_tolerance)}</td>
                  <td className="n">{formatDecimal(user.mandate.exploration_share)}</td>
                  <td data-testid="mandate-families">
                    {user.mandate.permitted_families.any ? (
                      <Chip tone="info">any family</Chip>
                    ) : (
                      <span className="num">
                        {user.mandate.permitted_families.families.join(", ")}
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableWell>
      </div>

      <p className="mt-2">
        <Muted>
          Every figure is the platform&rsquo;s own text, grouped for reading and never rounded or
          added to another. Investable is the platform&rsquo;s{" "}
          <span className="num">capital − liquidity_floor</span>, not this page&rsquo;s: the money
          figures arrive as exact decimal strings and this console does not do arithmetic on money.
          There is no total row for the same reason.
        </Muted>
      </p>
      <p className="mt-1">
        <Muted>
          served {formatTimestamp(data.served_at)} · posture reported by the body: {data.posture}
        </Muted>
      </p>
    </>
  );
}

/**
 * The three facts about a mandate the platform holds and this route does not
 * carry, and the one route that does not exist.
 *
 * Stated on the page rather than left as an absence, because an absence and an
 * omission look identical from a screen. Each is checkable: the accessor named
 * is the one that holds the fact in `qip-capital`, and `MandateView` in
 * `qip-api/src/ledger_views.rs` is the projection that drops it.
 */
function NotProjected() {
  return (
    <div className="flex flex-col gap-3">
      <StateBlock
        tone="warn"
        label="not projected"
        headline="The route carries a mandate's terms and not the agreement's identity."
      >
        <ul className="flex list-disc flex-col gap-1.5 pl-4">
          <li data-testid="mandate-gap-id">
            <strong>No mandate id.</strong> The registry holds one —{" "}
            <span className="num">MandateRegistry::registration</span> answers a{" "}
            <span className="num">RegisteredMandate</span> carrying{" "}
            <span className="num">id</span> — and <span className="num">MandateView</span> does not
            project it. So this register can show what a mandate says and cannot name which
            agreement said it, which is the field an amendment would supersede and an audit would
            cite.
          </li>
          <li data-testid="mandate-gap-registered">
            <strong>No registration instant.</strong> The same record carries{" "}
            <span className="num">registered_at</span>, and it is not projected either. &ldquo;What
            did we agree, and when&rdquo; is one question; this page can answer half of it.
          </li>
          <li data-testid="mandate-gap-ceiling">
            <strong>No desk ceiling.</strong> Every user mandate was admitted under the desk&rsquo;s
            (<span className="num">MandateRegistry::desk_mandate</span>), and a registration is
            refused when it exceeds it on currency, capital, risk tolerance, exploration share, a
            family the desk does not permit, or the total under user mandates. The ceiling that made
            each row admissible is not in the body, so the register shows the terms without the test
            they passed.
          </li>
        </ul>
        <p className="mt-2">
          Nothing is filled in for any of the three. A console that displayed a mandate id it
          derived, or a date it guessed from <span className="num">served_at</span>, would be
          answering an audit question with its own invention.
        </p>
      </StateBlock>

      <StateBlock
        tone="neutral"
        label="no control"
        headline="No route creates, amends or retires a mandate, so this page offers nothing that would."
      >
        <p data-testid="mandate-provenance">
          A mandate reaches the ledger from the deployment&rsquo;s committed configuration —{" "}
          <span className="num">UserMandate</span> in the kernel&rsquo;s config, validated at
          assembly and refused rather than corrected — because, in that type&rsquo;s own words, a
          mandate invented at runtime would be capital the platform promised to somebody nobody
          named. <span className="num">qip-api</span> serves one write anywhere near this surface,{" "}
          <span className="num">POST /ledger/users/&#123;user&#125;/eligibility</span>, and it
          records an operator&rsquo;s finding about a person rather than changing an agreement. It
          lives on the ledger page, beside the list it changes.
        </p>
        <p className="mt-2">
          There is likewise no field here about capital leaving the platform. The eligibility record
          carries no withdrawal term at all — not a term that is always refused, but one that is not
          there — because ADR 0021 refuses the path it would describe and ADR 0023 keeps that in
          force.
        </p>
      </StateBlock>
    </div>
  );
}
