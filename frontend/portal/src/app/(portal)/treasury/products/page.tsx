"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { formatCount, formatTimestamp } from "@/lib/format";
import { useLedgerUsers, type LedgerUser } from "@/lib/hooks/useTreasury";
import { CapabilityChip, Muted, TreasuryHeader, WithdrawalChip } from "../_shared";

/**
 * Product entitlements (blueprint §40.13, §43.3) — the catalogue, and what the
 * platform decided each account may do with each product.
 *
 * The ledger page reads the same route down the other axis: one card per user,
 * with that user's entitlements inside it. That answers "what may Alice do".
 * It does not answer "who may invest in this family, and who was refused, and
 * why" — which is the question a desk asks before a family is offered anywhere,
 * and the question that has to be answered for every account at once or not at
 * all. This page is that pivot and nothing more: every grant, every refusal and
 * every reason on it is a field `GET /ledger/users` answered, regrouped by
 * `family`. No capability is computed here.
 *
 * Three things are deliberate.
 *
 * **The catalogue is the platform's, and an empty one says so.** `products` is
 * the set of strategy families registered with the central factory. When it is
 * empty this page says the platform registered none — which is a read that
 * succeeded and found nothing, and must never be confused with a read that did
 * not happen. `ResourceView` renders the unreachable, refused and missing cases
 * in their own visually distinct blocks; the empty catalogue is a fourth block
 * with its own label and its own words.
 *
 * **A user with no evaluation against a product is stated, not inferred.** The
 * route evaluates every product for every user, so a missing row means the
 * process answered something this console did not expect. Rendering it as
 * "refused" would be inventing a refusal nobody decided; rendering it as
 * granted would be worse. It is rendered as an absence, in those words.
 *
 * **Withdrawal is refused on every row and there is no control for it.**
 * `WithdrawalEntitlement` in `qip-capital` has exactly one variant, and the
 * eligibility record deliberately carries no withdrawal field at all — not a
 * field that is always false but a field that is not there — because ADR 0021
 * refuses the path it would describe and ADR 0023 keeps that in force. This
 * page renders the platform's refusal and its reason, and offers nothing that
 * could ask for the other answer.
 */
export default function ProductEntitlementsPage() {
  const ledger = useLedgerUsers();

  return (
    <div className="flex flex-col gap-3 p-3">
      <TreasuryHeader
        title="Product entitlements"
        reads="GET /ledger/users"
        posture={ledger.data?.posture ?? null}
        meta={<Freshness resource={ledger} name="entitlements" />}
      />

      <Panel>
        <PanelHead title="Products and what each account may do" />
        <PanelBody>
          <ResourceView resource={ledger} loadingRows={4}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Products registered"
                    value={<span data-testid="product-count">{formatCount(data.products.length)}</span>}
                    note="GET /ledger/users: products — the families the central factory holds"
                  />
                  <Kpi
                    label="Accounts evaluated"
                    value={<span data-testid="product-account-count">{formatCount(data.users.length)}</span>}
                    note="every user the ledger holds a mandate for"
                  />
                  <Kpi
                    label="Evaluated as role"
                    value={<span className="text-[15px]">{data.evaluated_as_role}</span>}
                    note="the platform evaluates this surface as the viewer role, for every account"
                  />
                  <Kpi
                    label="Withdrawal grants"
                    value={<span data-testid="product-withdrawal-grants">{formatCount(withdrawalGrants(data.users))}</span>}
                    note="the platform's type has no granted arm; anything but zero is a contradiction to investigate"
                    tone={withdrawalGrants(data.users) === 0 ? "neutral" : "bad"}
                  />
                </KpiRow>

                <p className="mt-2" data-testid="product-evaluation-note">
                  <Muted>
                    These are the platform&rsquo;s evaluations for the ledger&rsquo;s{" "}
                    {data.evaluated_as_role} role against a ledger user id — not for the person
                    signed in to this console. This console reaches the platform with one
                    deployment credential, a subject such as{" "}
                    <span className="num">operator@env</span> that is the same for every person who
                    signs in here, and the platform has no way to bind a console session to a
                    ledger account. Nothing on this page is scoped to you.
                  </Muted>
                </p>

                {data.products.length === 0 ? (
                  <div className="mt-3" data-testid="product-catalogue-empty">
                    <EmptyBlock headline="The platform has registered no product.">
                      <p>
                        <span className="num">GET /ledger/users</span> answered, and its{" "}
                        <span className="num">products</span> list is empty: no strategy family is
                        registered with the central factory, so there is nothing for an entitlement
                        to be evaluated against and the platform evaluated none. This is a catalogue
                        that was read and found empty — not a catalogue this console failed to read.
                        A platform that could not be reached, a credential that was refused and a
                        route that does not exist each say so in their own words above this line.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <div className="mt-3 flex flex-col gap-3">
                    {data.products.map((family) => (
                      <ProductCard key={family} family={family} users={data.users} />
                    ))}
                  </div>
                )}

                <p className="mt-2">
                  <Muted>
                    served {formatTimestamp(data.served_at)} · posture reported by the body:{" "}
                    {data.posture}
                  </Muted>
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
 * The platform's own count of granted withdrawals, which must be zero.
 *
 * Counted rather than assumed. `WithdrawalChip` already raises a per-row alarm
 * on a granted withdrawal; this puts the same fact where a reader looks first,
 * so a single contradicted row inside a long catalogue cannot be missed.
 */
function withdrawalGrants(users: readonly LedgerUser[]): number {
  return users.reduce(
    (total, user) => total + user.entitlements.filter((e) => e.can_withdraw.granted).length,
    0,
  );
}

function ProductCard({ family, users }: { family: string; users: readonly LedgerUser[] }) {
  const rows = users.map((user) => ({
    user,
    entitlement: user.entitlements.find((e) => e.family === family) ?? null,
  }));
  const granted = rows.filter((row) => row.entitlement?.can_invest.granted === true).length;

  return (
    <section
      className="flex flex-col gap-2 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2"
      data-testid="product-card"
      data-product={family}
      aria-label={`entitlements for ${family}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="num text-[14px] font-semibold" data-testid="product-family">
          {family}
        </span>
        <Chip tone="info">
          <span data-testid={`product-invest-grants-${family}`}>
            {formatCount(granted)} of {formatCount(rows.length)} may invest
          </span>
        </Chip>
      </div>

      {rows.length === 0 ? (
        <div data-testid={`product-no-accounts-${family}`}>
          <EmptyBlock headline="No account has been evaluated against this product.">
            <p>
              The platform registered this family and the ledger holds no mandate for anyone, so
              there is no account for an entitlement to be decided about. A family with no account
              is not a family nobody may invest in; it is a family nobody has been asked about.
            </p>
          </EmptyBlock>
        </div>
      ) : (
        <ul className="flex flex-col gap-2">
          {rows.map(({ user, entitlement }) => (
            <li
              key={user.user_id}
              className="grid gap-3 border-t border-[color:var(--color-line)] pt-2"
              style={{ gridTemplateColumns: "minmax(160px, 0.8fr) repeat(3, 1fr)" }}
              data-testid="product-entitlement-row"
              data-user={user.user_id}
            >
              <div className="flex flex-col gap-0.5">
                <span className="num text-[12px]">{user.user_id}</span>
                <Muted>
                  {user.mandate.jurisdiction} ·{" "}
                  {entitlement === null
                    ? "no evaluation answered"
                    : `${entitlement.role} · evaluated ${formatTimestamp(entitlement.evaluated_at)}`}
                </Muted>
              </div>
              {entitlement === null ? (
                <div className="col-span-3" data-testid="product-entitlement-absent">
                  <StateBlock
                    tone="warn"
                    label="not evaluated"
                    headline="The platform answered no entitlement for this account against this product."
                    compact
                  >
                    <p>
                      The route evaluates every registered product for every user it holds a mandate
                      for, so an account missing from a product it lists is a shape this console did
                      not expect. It is shown as an absence: not a refusal, which nobody decided,
                      and certainly not a grant.
                    </p>
                  </StateBlock>
                </div>
              ) : (
                <>
                  <CapabilityChip label="can view" capability={entitlement.can_view} />
                  <CapabilityChip label="can invest" capability={entitlement.can_invest} />
                  <WithdrawalChip entitlement={entitlement.can_withdraw} />
                </>
              )}
            </li>
          ))}
        </ul>
      )}

      <p data-testid={`product-withdrawal-note-${family}`}>
        <Muted>
          No account may withdraw from this or any product, and there is no control here that could
          ask. The platform&rsquo;s withdrawal type has one arm, and the eligibility record carries
          no withdrawal field at all — not a field that is always false, but a field that is not
          there. ADR 0021 refuses the path it would describe; ADR 0023 keeps that in force.
        </Muted>
      </p>
    </section>
  );
}
