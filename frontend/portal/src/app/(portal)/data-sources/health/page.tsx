"use client";

import Link from "next/link";
import type { ReactNode } from "react";
import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import {
  EmptyBlock,
  MissingEndpointBlock,
  ResourceView,
  StateBlock,
} from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { SystemStatus } from "@/lib/api/types";
import { formatTimestamp } from "@/lib/format";
import { useRegistrations, type RegistrationSource } from "@/lib/hooks/useRegistrations";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Per-source health: the latency, freshness, quality and provenance of every
 * feed the platform reads — none of which the platform serves.
 *
 * The feed catalogue at `/data-sources` names the gap in one block beside the
 * facts it *can* show. This page is the gap itself, at the level of detail an
 * operator asking "is this source good right now" needs, and its whole job is
 * to say which of the four facts exists, which does not, and which nearby fact
 * is a different fact rather than a stand-in.
 *
 * **What `qip-api` actually serves, checked rather than remembered.**
 * `backend/crates/apps/qip-api/src/routes.rs` matches `(Method::Get,
 * "/data-sources")` to exactly one expression —
 * `Response::json(200, unavailable("sources", crate::missing::NO_DATA_FINDER))`
 * — and `unavailable` builds
 * `{"subject":…,"available":false,"reason":…}` and nothing else. There is no
 * second arm: the route does not branch on whether a finder is composed in, so
 * no deployment of this process has ever answered a source list, let alone a
 * latency, a freshness instant, a quality score or a lineage record. The route
 * table's own summary for it ("discovered, approved and rejected data sources
 * with health and licensing") describes the surface somebody intends, not the
 * body the handler returns, and a console that trusted the summary would be
 * promising four fields that no code path can produce.
 *
 * So nothing here is filled in. The four facts are listed one row each with the
 * route that would carry them, and every row says the same thing in the same
 * place: no field. The catalogue below it is real — `GET /registrations` is
 * served and answers the sources the finder catalogues — and it is shown with a
 * health column that is empty for every row, because a per-source health record
 * is exactly what is missing and an operator should be able to read that off
 * one screen rather than infer it from a page that has no health panel at all.
 *
 * **What this page will not render, and why it is a rule rather than a habit.**
 * The gateway redacts `QIP_API_BASE_URL` out of its own error bodies so an
 * upstream address does not reach a browser. Anything this console says about
 * an upstream is held to the same line: no credential, no variable a credential
 * is read from, no command that would write one, no venue URL and no internal
 * address. `GET /registrations` carries `secret_slot`, `secret_command`,
 * `terms` and the companion slots, and none of the four is rendered here.
 *
 * There is no control on this page. It reads, and a page about whether a feed
 * is healthy has nothing it could legitimately submit.
 */
export default function DataSourceHealth() {
  const catalogue = useRegistrations();
  const registry = useResource<unknown>(platform.dataSources, {
    key: "feed-health-registry",
    label: "GET /data-sources",
    intervalMs: 60_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <SurfaceHeader posture={catalogue.data?.posture ?? null} meta={<Freshness resource={catalogue} name="catalogued sources" />} />

      <Panel data-testid="feed-health-missing">
        <PanelHead
          title="The health document, and the fact that there is not one"
          actions={<Chip tone="warn">GET /api/v1/data-sources/health</Chip>}
        />
        <PanelBody>
          <MissingEndpointBlock endpoint={NOT_YET_SERVED["dataSourceHealth"]!} />
          <p
            className="mt-2 max-w-[86ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
            data-testid="feed-health-verified"
          >
            Checked against the process rather than assumed:{" "}
            <span className="num">routes.rs</span> matches{" "}
            <span className="num">(Method::Get, &quot;/data-sources&quot;)</span> to a single
            expression, <span className="num">unavailable(&quot;sources&quot;, NO_DATA_FINDER)</span>
            , which serialises{" "}
            <span className="num">
              &#123;&quot;subject&quot;,&quot;available&quot;:false,&quot;reason&quot;&#125;
            </span>{" "}
            and no other key. The handler has no second arm, so this is not a body that grows health
            fields once a data finder is composed in — there is no branch for one to take.
          </p>
        </PanelBody>
      </Panel>

      <Panel data-testid="feed-health-facts">
        <PanelHead title="The four facts, one row each" />
        <PanelBody flush>
          <TableWell label="the four per-source health facts and whether any route carries them">
            <table className="dt">
              <thead>
                <tr>
                  <th scope="col">Fact</th>
                  <th scope="col">Route that would carry it</th>
                  <th scope="col">What this deployment serves instead</th>
                  <th scope="col">Standing</th>
                </tr>
              </thead>
              <tbody>
                {FACTS.map((fact) => (
                  <tr key={fact.id} data-testid="feed-health-fact" data-fact={fact.id} data-alert="true">
                    <td className="num">{fact.label}</td>
                    <td className="num text-[color:var(--color-ink-dim)]">
                      GET /api/v1/data-sources/health
                    </td>
                    <td className="max-w-[62ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                      {fact.instead}
                    </td>
                    <td>
                      <Chip tone="warn">
                        <span data-testid={`feed-health-standing-${fact.id}`}>no field</span>
                      </Chip>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </TableWell>
          <p className="border-t border-[color:var(--color-line)] px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            Two of those rows name a fact that is nearby and is not the same fact. Cell report age is
            how old an edge cell&rsquo;s report is; source freshness is how old the data underneath
            it is, and a cell can report on the minute about a feed that stopped yesterday.
            Registration standing is a licensing fact about who holds an account with a venue; it
            says nothing about whether the venue answered. Neither is shown in a health column here,
            because a nearby number in the place a missing one belongs is the failure this console
            exists to avoid.
          </p>
        </PanelBody>
      </Panel>

      <Panel data-testid="feed-health-catalogue">
        <PanelHead
          title="Catalogued sources, and the health record each carries"
          meta={<Freshness resource={catalogue} name="catalogued sources" />}
          actions={
            <>
              <Chip>GET /api/v1/registrations</Chip>
              <Link className="btn" data-variant="ghost" href="/data-sources">
                Feed catalog
              </Link>
            </>
          }
        />
        <PanelBody flush>
          <p className="border-b border-[color:var(--color-line)] px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            The sources the finder catalogues, read from the one route that answers them. The health
            column is empty on every row and that is the finding, not a loading state: no route in
            this platform carries a latency, a freshness instant, a quality score or a lineage record
            for any of them.
          </p>
          <ResourceView resource={catalogue} loadingRows={4}>
            {(data) =>
              data.sources.length === 0 ? (
                <div className="p-3" data-testid="feed-health-none">
                  <EmptyBlock headline="The platform catalogues no source at all.">
                    <p>
                      <span className="num">GET /api/v1/registrations</span> answered and its{" "}
                      <span className="num">sources</span> list is empty. There is no feed to be
                      healthy or unhealthy in this deployment — a read that succeeded and found
                      nothing, which is a different fact from a platform this console could not
                      reach and a different fact again from a credential the route refused. Each of
                      those says so in its own words and its own colour.
                    </p>
                  </EmptyBlock>
                </div>
              ) : (
                <>
                  <TableWell maxHeight="38vh" label="catalogued sources and their health record">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Source</th>
                          <th scope="col">Latency</th>
                          <th scope="col">Freshness</th>
                          <th scope="col">Quality</th>
                          <th scope="col">Provenance</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.sources.map((source) => (
                          <SourceRow key={source.source_id} source={source} />
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                  <p className="border-t border-[color:var(--color-line)] px-3 py-2 text-[11px] text-[color:var(--color-ink-faint)]">
                    catalogue served {formatTimestamp(data.served_at)} · posture reported by the
                    body: {data.posture}
                  </p>
                </>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel data-testid="feed-health-registry">
        <PanelHead
          title="The registry route's own answer"
          meta={<Freshness resource={registry} name="source registry" />}
          actions={<Chip>GET /api/v1/data-sources</Chip>}
        />
        <PanelBody>
          <ResourceView resource={registry} loadingRows={3}>
            {(data) => (
              <StateBlock
                tone="warn"
                label="unmodelled body"
                headline="The registry answered a body this console does not model."
              >
                <p>
                  Its top-level keys are{" "}
                  <span className="num" data-testid="feed-health-registry-keys">
                    {topLevelKeys(data)}
                  </span>
                  . The keys are shown and the values are not: a registry body may name an upstream
                  host or a path this browser has no business holding, and the gateway redacts{" "}
                  <span className="num">QIP_API_BASE_URL</span> from its own errors for the same
                  reason. If a health field appears among those keys, this page is out of date with
                  the platform and should be extended rather than trusted.
                </p>
              </StateBlock>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel data-testid="feed-health-redaction">
        <PanelHead title="What this page will not render" />
        <PanelBody>
          <StateBlock
            tone="neutral"
            label="withheld on purpose"
            headline="No credential, no variable name, no command, no venue URL, no internal address."
          >
            <p>
              <span className="num">GET /registrations</span> carries{" "}
              <span className="num">secret_slot</span>, <span className="num">secret_command</span>,{" "}
              <span className="num">terms</span> and any companion slots, and none of the four is on
              this page. A health screen is the one an operator screenshots into an incident thread,
              and a variable name beside a venue is half of the instruction for getting a key out of
              a deployment. The variable names live on{" "}
              <Link href="/data-sources/registrations" className="underline">
                venue registrations
              </Link>
              , behind the page whose job is the credential lifecycle.
            </p>
            <p className="mt-2">
              Nothing here reaches the platform except reads, and no control on this page could
              submit an order: none exists, and the gateway declares no write this page could call.
            </p>
          </StateBlock>
        </PanelBody>
      </Panel>
    </div>
  );
}

/** The four facts this surface is named for, and what the platform has instead of each. */
const FACTS = [
  {
    id: "latency",
    label: "Latency",
    instead:
      "Nothing per source. The feed catalogue measures this browser's own round trip to the gateway, which is this console's latency to qip-api and not a source's latency to the venue.",
  },
  {
    id: "freshness",
    label: "Freshness",
    instead:
      "GET /regions carries a report age per edge cell. That is how old a cell's report is, not how old a source's data is; it is a different fact and is not shown in this fact's place.",
  },
  {
    id: "quality",
    label: "Quality",
    instead:
      "Nothing. No route in this platform scores a source, so there is no number to grade and none is invented here.",
  },
  {
    id: "provenance",
    label: "Provenance",
    instead:
      "Nothing per source. GET /registrations names which venue demands an account and who registered with it — a licensing record about people, not a lineage record about data.",
  },
] as const;

/**
 * One catalogued source, with the health record it does not have.
 *
 * Four cells rather than one saying "no health": an operator scanning for the
 * column that broke needs the columns to exist. The source id is the platform's
 * own catalogue key and is safe to show; nothing else from the row is.
 */
function SourceRow({ source }: { source: RegistrationSource }) {
  return (
    <tr data-testid="feed-health-source-row" data-source={source.source_id} data-alert="true">
      <td className="num">{source.source_id}</td>
      {(["latency", "freshness", "quality", "provenance"] as const).map((fact) => (
        <td key={fact}>
          <span
            className="chip"
            data-tone="warn"
            data-testid={`feed-health-cell-${fact}`}
            title="GET /api/v1/data-sources/health is not served, so no route carries this field for any source."
          >
            no record
          </span>
        </td>
      ))}
    </tr>
  );
}

/** The keys a body carried, sorted, without any of its values. */
function topLevelKeys(body: unknown): string {
  if (typeof body !== "object" || body === null) return `a ${typeof body}, with no keys`;
  const keys = Object.keys(body as Record<string, unknown>).sort();
  return keys.length === 0 ? "none" : keys.join(", ");
}

/**
 * The posture declaration, on the page and not only in the chrome.
 *
 * Two reports and no assumption, the same pair the treasury surface shows: the
 * catalogue body's own `posture` literal once it has landed, and the platform's
 * live capability from `GET /system/status`. The static `PAPER TRADING` label
 * beside them is a statement about this console — it reads and cannot trade —
 * and it renders whether or not either read answers, because the read failing
 * is exactly when an operator needs to know what this screen can do.
 */
function SurfaceHeader({ posture, meta }: { posture: string | null; meta: ReactNode }) {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "feed-health-status",
    label: "GET /system/status",
    intervalMs: 15_000,
  });

  return (
    <Panel data-testid="feed-health-header">
      <PanelHead
        title="Feed health"
        meta={meta}
        actions={
          <>
            {posture === null ? null : (
              <Chip tone={posture === "PAPER TRADING" ? "ok" : "bad"} title="GET /registrations: posture">
                <span data-testid="feed-health-body-posture">{posture}</span>
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
        <p
          className="max-w-[92ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
          data-testid="feed-health-declaration"
        >
          <span className="chip mr-2" data-tone="ok" data-testid="feed-health-paper-label">
            PAPER TRADING
          </span>
          This page reads <span className="num">GET /registrations</span>,{" "}
          <span className="num">GET /data-sources</span> and{" "}
          <span className="num">GET /system/status</span> and renders what they answered. There is no
          control on it, nothing here can submit an order, and no per-source health figure is shown
          because the platform serves none.
        </p>
      </PanelBody>
    </Panel>
  );
}
