"use client";

import { useMemo } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Bars } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { OpenApiDocument } from "@/lib/api/types";
import { formatCount, truncate } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The role each route requires, read from the live route table.
 *
 * There is no user store behind this page because the platform has none:
 * identity is a bearer token per role, delivered by deployment configuration.
 * So there are no users to list and no accounts to manage — only the roles the
 * route table itself declares, read from the OpenAPI document the process
 * generates from its own routes at the moment of the request. A page that
 * listed roles from a constant in this repository would keep advertising an
 * authority the platform had stopped requiring, and would be believed.
 */

/**
 * The platform's role ladder, least authority first. Roles accumulate — the
 * document's own operation descriptions say a role above may call everything
 * below — so the order here is the platform's, not a display preference.
 */
const LADDER = ["monitor", "viewer", "analyst", "operator"] as const;

function rankOf(role: string): number {
  const index = LADDER.indexOf(role as (typeof LADDER)[number]);
  // A role the ladder does not name sorts after every role it does: the page
  // must show it rather than hide it, but must not guess where it sits.
  return index === -1 ? LADDER.length : index;
}

interface Route {
  readonly method: string;
  readonly path: string;
  readonly role: string;
  readonly summary: string;
}

interface RoleRow {
  readonly role: string;
  readonly routes: number;
  readonly examples: readonly string[];
}

function routesOf(document: OpenApiDocument): readonly Route[] {
  const routes: Route[] = [];
  for (const [path, operations] of Object.entries(document.paths)) {
    for (const [method, operation] of Object.entries(operations)) {
      routes.push({
        method: method.toUpperCase(),
        path,
        // "unstated" is a fact worth surfacing: a route that declares no role
        // is a review item, not a row to drop.
        role: operation["x-required-role"] ?? "unstated",
        summary: operation.summary ?? operation.operationId ?? "",
      });
    }
  }
  return routes.sort(
    (a, b) =>
      rankOf(a.role) - rankOf(b.role) ||
      a.role.localeCompare(b.role) ||
      a.path.localeCompare(b.path) ||
      a.method.localeCompare(b.method),
  );
}

function rolesOf(routes: readonly Route[]): readonly RoleRow[] {
  const byRole = new Map<string, Route[]>();
  for (const route of routes) {
    const bucket = byRole.get(route.role) ?? [];
    bucket.push(route);
    byRole.set(route.role, bucket);
  }
  return [...byRole.entries()]
    .sort((a, b) => rankOf(a[0]) - rankOf(b[0]) || a[0].localeCompare(b[0]))
    .map(([role, owned]) => ({
      role,
      routes: owned.length,
      examples: owned.slice(0, 3).map((route) => `${route.method} ${route.path}`),
    }));
}

export default function AccessPage() {
  const document = useResource<OpenApiDocument>(platform.openapi, {
    key: "access-openapi",
    label: "GET /openapi.json",
    intervalMs: 60_000,
  });

  const routes = useMemo(
    () => (document.data === null ? [] : routesOf(document.data)),
    [document.data],
  );
  const roles = useMemo(() => rolesOf(routes), [routes]);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Roles the route table declares"
          meta={<Freshness resource={document} name="the route table" />}
        />
        <PanelBody>
          <ResourceView resource={document} loadingRows={2}>
            {() => (
              <>
                <KpiRow>
                  <Kpi
                    label="Roles"
                    value={formatCount(roles.length)}
                    note="distinct roles named by the live route table"
                  />
                  <Kpi
                    label="Routes"
                    value={formatCount(routes.length)}
                    note="operations declaring a required role"
                  />
                  {roles.map((row) => (
                    <Kpi
                      key={row.role}
                      label={row.role}
                      value={formatCount(row.routes)}
                      unit="routes"
                      note={truncate(row.examples.join(" · "), 64)}
                    />
                  ))}
                </KpiRow>
                <p className="mt-2 max-w-[90ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                  The ladder runs monitor &lt; viewer &lt; analyst &lt; operator, and roles
                  accumulate: the document&rsquo;s own operation descriptions state that a role
                  above may call everything a role below may. A count against
                  &ldquo;viewer&rdquo; is therefore the routes that <em>first</em> become
                  callable at viewer, not the whole surface a viewer token can reach.
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[1fr_2fr]">
        <Panel>
          <PanelHead title="Routes per role" />
          <PanelBody>
            {roles.length === 0 ? (
              <EmptyBlock headline="The document declares no role yet." />
            ) : (
              <Bars
                items={roles.map((row) => ({
                  label: row.role,
                  value: row.routes,
                  tone: "accent" as const,
                }))}
                unit=" routes"
              />
            )}
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="Every route, by the role it requires"
            meta={<Freshness resource={document} name="routes" />}
          />
          <PanelBody flush>
            <ResourceView resource={document} loadingRows={10}>
              {() =>
                routes.length === 0 ? (
                  <EmptyBlock headline="The document declares no path." />
                ) : (
                  <TableWell maxHeight="520px" label="Routes by required role">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Role</th>
                          <th scope="col">Method</th>
                          <th scope="col">Path</th>
                          <th scope="col">Summary</th>
                        </tr>
                      </thead>
                      <tbody>
                        {routes.map((route) => (
                          <tr key={`${route.method} ${route.path}`}>
                            <td>
                              <Chip tone={route.role === "unstated" ? "warn" : "neutral"}>
                                {route.role}
                              </Chip>
                            </td>
                            <td className="num">{route.method}</td>
                            <td className="num">{route.path}</td>
                            <td className="whitespace-normal text-[11.5px]">{route.summary}</td>
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
      </div>

      <Panel>
        <PanelHead title="Why there are no users here" />
        <PanelBody>
          <p className="max-w-[90ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
            The platform has no user store. Identity is a bearer token per role, delivered by
            deployment configuration, so there are no users to manage on this page — only the
            roles the route table itself declares. <span className="num">GET /api/v1/users</span>{" "}
            does not exist, and this console does not pretend it does: everything above is read
            from <span className="num">GET /openapi.json</span>, and nothing on this page can
            create, edit, or revoke anything.
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}
