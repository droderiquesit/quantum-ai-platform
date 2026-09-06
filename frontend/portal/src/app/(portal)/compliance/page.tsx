"use client";

import Link from "next/link";
import { Chip, Freshness, KeyValue, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import {
  EmptyBlock,
  MissingEndpointBlock,
  ResourceView,
  StateBlock,
} from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { Autonomy, Governance, SystemView } from "@/lib/api/types";
import { formatCount, formatTimestamp } from "@/lib/format";
import {
  slotAccess,
  useRegistrations,
  useRegistrationSlots,
  SLOTS_REFUSAL,
  SLOTS_ROUTE,
  type RegistrationSlotSource,
  type RegistrationSource,
  type SlotAccess,
} from "@/lib/hooks/useRegistrations";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The compliance surface: what this deployment is obliged to do before it may
 * read a venue, what its own governance review found, what its record can be
 * held to, and — said in the console's own vocabulary for an absence — the
 * obligations register it does not have.
 *
 * **Why this page is not `GET /compliance`.** There is no such route.
 * `backend/crates/apps/qip-api/src/routes.rs` declares 27 reads and five
 * writes and none of them is a compliance document; `NOT_YET_SERVED` has said
 * so since the table existed, and the entry is rendered here rather than
 * paraphrased. What the platform *does* serve are the four facts a compliance
 * question in this system actually reduces to, and each has a route of its
 * own:
 *
 * * **Licensing and venue obligations** — `GET /registrations`. Per catalogued
 *   source: what the venue demands before it may be read, whether a named
 *   person has registered and when they read the terms, and the terms
 *   reference itself. `qip-data-finder` evaluates licensing posture *before* a
 *   source reaches the catalogue, so a source that is pending here is a source
 *   the platform is refusing to read — the obligation is enforced upstream and
 *   this page reports it.
 * * **Governance findings** — `GET /system/governance`, the review of the
 *   agent roster.
 * * **Auditability** — `GET /system`, whose `chain_intact` is the hash chain
 *   re-walked on this read, and `events_logged` the length of it.
 * * **Posture and its change record** — `GET /autonomy`: the level, the
 *   ceiling, and every change with the operator who asked for it and why.
 *
 * **Nothing here is computed and nothing here is a control.** Every field is
 * rendered as the platform sent it; no obligation is inferred from another, no
 * standing is derived, no attestation is synthesised. There is no button on
 * this page at all. The one write the registration surface has — an operator
 * recording that they read a venue's terms — lives on `/data-sources/registrations`
 * beside the card it changes, and this page links to it rather than reproducing
 * it, because a page about obligations that could also discharge one is a page
 * where the two are hard to tell apart.
 *
 * **Four states, four blocks.** Nothing has arrived yet; the route answered
 * and the register is genuinely empty; the platform could not be reached; and
 * the console's credential may not read the route. `ResourceView` gives each
 * its own words and its own colour. A compliance surface that rendered "no
 * obligations" when it had failed to ask would be the worst screen in this
 * console.
 */
export default function CompliancePage() {
  const registrations = useRegistrations();
  // The credential-slot column's own read. It is a second route at a role
  // this console does not hold, so the column states its absence per row
  // rather than showing an em dash — which on this table already means "the
  // manifest names no variable" and would otherwise absorb a refusal into a
  // fact about the venue.
  const slots = useRegistrationSlots();
  const access = slotAccess(slots);
  const governance = useResource<Governance>(platform.governance, {
    key: "compliance-governance",
    label: "GET /system/governance",
    intervalMs: 30_000,
  });
  const system = useResource<SystemView>(platform.system, {
    key: "compliance-system",
    label: "GET /system",
    intervalMs: 15_000,
  });
  const autonomy = useResource<Autonomy>(platform.autonomy, {
    key: "compliance-autonomy",
    label: "GET /autonomy",
    intervalMs: 20_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="compliance-page">
      <Panel>
        <PanelHead
          title="Compliance"
          meta={<Freshness resource={registrations} name="obligations" />}
          actions={
            autonomy.data === null ? null : (
              // Posture is shown on this panel, so the paper label is on this
              // panel. The banner above covers the screen and does not travel
              // with a panel lifted into a regulator's file; live-capable here
              // would be an alarm, not a mode.
              <StatusChip
                tone={autonomy.data.live ? "bad" : "ok"}
                label={autonomy.data.live ? "LIVE-CAPABLE" : "PAPER TRADING"}
                title="GET /autonomy: live"
              />
            )
          }
        />
        <PanelBody>
          <p
            className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
            data-testid="compliance-declaration"
          >
            <span className="chip mr-2" data-tone="ok" data-testid="compliance-paper-label">
              PAPER TRADING
            </span>
            This page reads <span className="num">GET /registrations</span>,{" "}
            <span className="num">GET /system/governance</span>, <span className="num">GET /system</span> and{" "}
            <span className="num">GET /autonomy</span> and renders what each answered. There is no
            control here: nothing on this page registers a venue, clears a finding, signs an
            attestation or submits an order, and the gateway declares no write this page could call.
          </p>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Autonomy, as the platform reports it"
          meta={<Freshness resource={autonomy} name="autonomy" />}
        />
        <PanelBody>
          <ResourceView resource={autonomy} loadingRows={3}>
            {(data) => (
              <div className="flex flex-col gap-3" data-testid="compliance-autonomy">
                <KpiRow>
                  <Kpi
                    label="Autonomy"
                    value={<span data-testid="compliance-autonomy-level">{data.level}</span>}
                    note="the level this process is running at"
                  />
                  <Kpi
                    label="Ceiling"
                    value={<span data-testid="compliance-autonomy-ceiling">{data.ceiling}</span>}
                    note="the highest level the composition root would admit; a live value stops the process rather than being lowered"
                  />
                  <Kpi
                    label="Live"
                    value={<span data-testid="compliance-autonomy-live">{data.live ? "yes" : "no"}</span>}
                    tone={data.live ? "bad" : "ok"}
                    note="whether autonomy permits live trading"
                  />
                  <Kpi
                    label="Changes recorded"
                    value={
                      <span data-testid="compliance-autonomy-changes">
                        {formatCount(data.history.length)}
                      </span>
                    }
                    note="since this process started; each carries the operator who asked and the reason"
                  />
                </KpiRow>
                {data.history.length === 0 ? (
                  <div data-testid="compliance-autonomy-history-empty">
                    <EmptyBlock headline="Autonomy has not changed since this process started.">
                      <p>
                        <span className="num">GET /autonomy</span> answered and its{" "}
                        <span className="num">history</span> is empty. That is a change record with
                        nothing in it — a read that succeeded — and not a record this console failed
                        to obtain. The record does not survive the process either way: changes are
                        in the event log, and this list is what one process has seen.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <TableWell maxHeight="26vh" label="Autonomy changes">
                    <table className="dt" data-testid="compliance-autonomy-history">
                      <thead>
                        <tr>
                          <th scope="col">At (ns since epoch)</th>
                          <th scope="col">From</th>
                          <th scope="col">To</th>
                          <th scope="col">Operator</th>
                          <th scope="col">Reason</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.history.map((change, index) => (
                          <tr key={`${change.at}-${index}`} data-testid="compliance-autonomy-row">
                            <td className="num text-[10px]">{change.at}</td>
                            <td className="num">{change.from}</td>
                            <td className="num">{change.to}</td>
                            <td className="num">{change.operator}</td>
                            <td className="whitespace-normal text-[11.5px]">{change.reason}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                )}
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Licensing and venue obligations"
          meta={<Freshness resource={registrations} name="registrations" />}
          actions={
            registrations.data === null ? null : (
              <Chip title="GET /registrations: posture">
                <span data-testid="compliance-body-posture">{registrations.data.posture}</span>
              </Chip>
            )
          }
        />
        <PanelBody>
          <ResourceView resource={registrations} loadingRows={5}>
            {(data) => (
              <div className="flex flex-col gap-3">
                {data.sources.length === 0 ? (
                  <div data-testid="compliance-obligations-empty">
                    <EmptyBlock headline="The catalogue holds no source.">
                      <p>
                        <span className="num">GET /registrations</span> answered and its{" "}
                        <span className="num">sources</span> list is empty: the data finder&rsquo;s
                        catalogue carries nothing, so there is no venue this deployment is obliged
                        to register with and none it may read. This is an observed empty catalogue —
                        a read that succeeded and found nothing — and not a platform this console
                        failed to reach, which is a different fact with a different remedy and says
                        so in its own words and its own colour.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <>
                    <KpiRow>
                      <Kpi
                        label="Sources catalogued"
                        value={
                          <span data-testid="compliance-source-count">
                            {formatCount(data.sources.length)}
                          </span>
                        }
                        note="every source the finder's catalogue carries, in catalogue order"
                      />
                      <Kpi
                        label="Awaiting a registration"
                        value={
                          <span data-testid="compliance-pending-count">
                            {formatCount(
                              data.sources.filter((s) => s.standing.standing === "pending").length,
                            )}
                          </span>
                        }
                        tone={
                          data.sources.some((s) => s.standing.standing === "pending") ? "warn" : "ok"
                        }
                        note="counted from the standings the platform answered; the feed's own gate refuses each of these until somebody registers"
                      />
                    </KpiRow>
                    <TableWell maxHeight="42vh" label="Venue obligations">
                      <table className="dt" data-testid="compliance-obligations">
                        <thead>
                          <tr>
                            <th scope="col">Source</th>
                            <th scope="col">The venue demands</th>
                            <th scope="col">Standing</th>
                            <th scope="col">Terms cited</th>
                            <th scope="col">Credential slot</th>
                          </tr>
                        </thead>
                        <tbody>
                          {data.sources.map((source) => (
                            <ObligationRow
                              key={source.source_id}
                              source={source}
                              slot={
                                access.status === "available"
                                  ? access.bySource.get(source.source_id) ?? null
                                  : null
                              }
                              access={access}
                            />
                          ))}
                        </tbody>
                      </table>
                    </TableWell>
                  </>
                )}
                <p
                  className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]"
                  data-testid="compliance-obligations-note"
                >
                  A credential slot is a deployment variable <em>name</em>. No value is in this
                  console, in the body it read, or in the process that served it, and none is shown.
                  Registering is an act a person performs and records; it is done on{" "}
                  <Link className="underline" href="/data-sources/registrations">
                    venue registrations
                  </Link>
                  , not here.
                </p>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Governance findings"
          meta={<Freshness resource={governance} name="governance" />}
          actions={
            governance.data === null ? null : (
              <Chip>
                <span data-testid="compliance-governance-agents">
                  {formatCount(governance.data.agents)}
                </span>
                &nbsp;agent(s) reviewed
              </Chip>
            )
          }
        />
        <PanelBody flush>
          <ResourceView resource={governance} loadingRows={3}>
            {(data) =>
              data.findings.length === 0 ? (
                <div className="p-3" data-testid="compliance-governance-empty">
                  <EmptyBlock headline="The agent roster raises no governance finding.">
                    <p>
                      Observed, not assumed:{" "}
                      <code className="num">GET /api/v1/system/governance</code> reviewed{" "}
                      {formatCount(data.agents)} agent(s) and returned no finding. A measured clean
                      bill — not a review that could not be run.
                    </p>
                  </EmptyBlock>
                </div>
              ) : (
                <TableWell maxHeight="32vh" label="Governance findings">
                  <table className="dt" data-testid="compliance-governance">
                    <thead>
                      <tr>
                        <th scope="col">Severity</th>
                        <th scope="col">Rule</th>
                        <th scope="col">Detail</th>
                        <th scope="col">Agents</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.findings.map((finding, index) => (
                        <tr
                          key={`${finding.rule}-${index}`}
                          data-testid="compliance-governance-row"
                          data-alert={finding.severity === "error" ? "true" : undefined}
                        >
                          <td>
                            <StatusChip
                              tone={finding.severity === "error" ? "bad" : "warn"}
                              label={finding.severity}
                            />
                          </td>
                          <td className="num">{finding.rule}</td>
                          <td className="whitespace-normal text-[11.5px]">{finding.detail}</td>
                          <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                            {finding.agents.join(", ") || "—"}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWell>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="What the record can be held to"
          meta={<Freshness resource={system} name="event chain" />}
        />
        <PanelBody>
          <ResourceView resource={system} loadingRows={3}>
            {(data) => (
              <div className="flex flex-col gap-3" data-testid="compliance-audit">
                <KpiRow>
                  <Kpi
                    label="Events logged"
                    value={
                      <span data-testid="compliance-events">{formatCount(data.events_logged)}</span>
                    }
                    note="records in the hash chain this process holds"
                  />
                  <Kpi
                    label="Chain intact"
                    value={
                      <span data-testid="compliance-chain">{data.chain_intact ? "yes" : "NO"}</span>
                    }
                    tone={data.chain_intact ? "ok" : "bad"}
                    note={
                      data.chain_intact
                        ? "every link re-walked on this read, not a stored attestation"
                        : "verification failed on this read"
                    }
                  />
                  <Kpi
                    label="Cycles"
                    value={<span data-testid="compliance-cycles">{formatCount(data.cycles)}</span>}
                    note="loop iterations logged"
                  />
                </KpiRow>
                {data.chain_intact ? null : (
                  <div data-testid="compliance-chain-broken">
                    <StateBlock
                      tone="bad"
                      label="chain broken"
                      headline={
                        data.chain_broken_at === null
                          ? "The event log's hash chain failed verification."
                          : `The event log's hash chain breaks at record ${formatCount(data.chain_broken_at)}.`
                      }
                    >
                      <p>
                        Every record after the break descends from a hash that does not match what
                        was sealed, so nothing decided since it can be attributed from the log
                        alone. Treat the log as compromised until the break is explained — this is
                        the condition the chain exists to make loud, and no compliance statement can
                        be made over a period that contains it.
                      </p>
                    </StateBlock>
                  </div>
                )}
                <dl style={{ maxWidth: "560px" }}>
                  <KeyValue label="Autonomy the log was written under" mono={false}>
                    {data.autonomy} (ceiling {data.ceiling})
                  </KeyValue>
                  <KeyValue label="Halted" mono={false}>
                    {data.halted
                      ? `yes — ${data.halted_scopes.join(", ") || "no scope named"}`
                      : "no"}
                  </KeyValue>
                </dl>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="What no route answers"
          actions={<Chip tone="warn">no endpoint</Chip>}
        />
        <PanelBody>
          <div className="flex flex-col gap-3" data-testid="compliance-absences">
            <MissingEndpointBlock endpoint={NOT_YET_SERVED["compliance"]!} />
            <MissingEndpointBlock endpoint={NOT_YET_SERVED["complianceAttestations"]!} />
            <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
              The panels above are the compliance facts this platform serves, each from its own
              route. They are not an obligations register and they are not an attestation: nothing
              here was signed by a person, covers a stated period, or was produced against a named
              obligation. Until those routes exist this console must not present the panels above as
              either, and it does not — nothing is filled in to stand in for them.
            </p>
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}

/**
 * One catalogued source's obligation, rendered field for field.
 *
 * The standing is the platform's tagged enum and each arm is rendered as its
 * own arm. A console that collapsed `pending` and a `null` requirement into
 * "keyless" would report a source as needing nothing when what actually
 * happened is that nobody declared what it needs — and the feed's gate refuses
 * it either way, so the screen and the gate would disagree.
 */
function ObligationRow({
  source,
  slot,
  access,
}: {
  source: RegistrationSource;
  slot: RegistrationSlotSource | null;
  access: SlotAccess;
}) {
  const standing = source.standing;
  return (
    <tr
      data-testid="compliance-obligation-row"
      data-source={source.source_id}
      data-standing={standing.standing}
      data-alert={standing.standing === "pending" ? "true" : undefined}
    >
      <td className="num">{source.source_id}</td>
      <td data-testid="compliance-requirement">
        {source.requirement ?? (
          <span className="text-[color:var(--color-warn)]">none declared</span>
        )}
      </td>
      <td data-testid="compliance-standing">
        {standing.standing === "keyless" ? (
          <Chip tone="ok">keyless — no registration required</Chip>
        ) : standing.standing === "registered" ? (
          <span className="flex flex-col gap-0.5">
            <Chip tone="ok">registered</Chip>
            <span className="text-[10.5px] leading-snug text-[color:var(--color-ink-faint)]">
              by <span className="num">{standing.operator}</span>, terms read{" "}
              {formatTimestamp(standing.terms_read_at)}
            </span>
          </span>
        ) : (
          <span className="flex flex-col gap-0.5">
            <Chip tone="warn">pending</Chip>
            <span className="text-[10.5px] leading-snug text-[color:var(--color-ink-faint)]">
              {standing.who_must_register} must register. {standing.reason}
            </span>
          </span>
        )}
      </td>
      <td className="whitespace-normal text-[11px]" data-testid="compliance-terms">
        {source.terms ?? <span className="text-[color:var(--color-ink-faint)]">none cited</span>}
      </td>
      <td
        className="num text-[10.5px]"
        data-testid="compliance-slot"
        data-slot-access={slot === null ? access.status : "available"}
      >
        {slot === null ? (
          <span className="whitespace-normal text-[color:var(--color-warn)]">
            {access.status === "refused"
              ? // Never the em dash. On this column an em dash means the
                // manifest names no credential variable, and a refusal shown
                // as one would read as a source that needs no key — the
                // opposite of what a refusal says, on the page whose job is
                // to be right about what each source is allowed to be read
                // under.
                `withheld: ${SLOTS_ROUTE} answered ${access.status_code}. ${SLOTS_REFUSAL}`
              : access.status === "loading"
                ? `reading ${SLOTS_ROUTE}…`
                : access.status === "unavailable"
                  ? `${SLOTS_ROUTE} did not answer: ${access.detail}`
                  : `${SLOTS_ROUTE} was served and carries no row for this source; both lists come from one catalogue, so this console will not guess which is right`}
          </span>
        ) : (
          <>
            {slot.secret_slot ?? "—"}
            {slot.companion_secret_slots.length > 0 ? (
              <span className="block text-[color:var(--color-ink-faint)]">
                {slot.companion_secret_slots.map((companion) => companion.variable).join(", ")}
              </span>
            ) : null}
          </>
        )}
      </td>
    </tr>
  );
}
