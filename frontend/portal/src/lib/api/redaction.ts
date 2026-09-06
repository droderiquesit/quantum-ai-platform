/**
 * What the platform puts on the wire that this console removes before a
 * browser sees it, and what it hands over anyway — with the reason for each.
 *
 * The gateway's rule used to be that a body crosses untouched, and the rule
 * was right about status codes and wrong about one class of field. `GET /mesh`
 * and `GET /system/status` both serve `cells[].address` at `Role::Viewer`:
 * the base URL a cell is identified by on the mesh transport, configured
 * through `QIP_MESH_PEER` (`qip-api/src/mesh.rs`, "the address is therefore
 * the identity"). Two console pages said in so many words that no upstream
 * address reaches a browser. Neither rendered one, and both were describing
 * pixels: the body in the network tab carried
 * `"cells":[{"cell":"eu-west-1","address":"127.0.0.1:34561",…}]`, where any
 * extension with host permissions and any client-side error reporter could
 * read it, and an operator screenshotting the page believed the claim.
 *
 * A page asserting a transport property the transport does not have is worse
 * than one that says nothing, because it is read as an assurance. So this
 * module holds the two halves of the true answer:
 *
 * * {@link WIRE_REDACTIONS} — fields the gateway replaces with
 *   {@link REDACTED} before the response leaves this process, naming each in
 *   the `x-qip-redacted` response header so the removal is visible in the
 *   same network tab rather than being a silent rewrite.
 * * {@link WIRE_DISCLOSURES} — fields the platform serves that this console
 *   does **not** remove, because a page here renders them by design. Removing
 *   those at the gateway would break the page that needs them while keeping
 *   nothing from a browser that can call the route itself; the fix is on the
 *   platform's side and is named per entry, for the lane that owns it.
 *
 * Both tables are rendered by the pages that used to make the claim in prose,
 * so what a page says about the wire is derived from what the gateway does to
 * it and cannot drift from it. `tests/wire.spec.ts` asserts on the response
 * body and header rather than on the DOM: a test that only reads the screen is
 * exactly what let the false claim ship.
 *
 * **Scope.** REST only. The five SSE channels are built by `stream.rs` in
 * `qip-api`, whose frames carry no `MeshCellStatus` — `grep -n address
 * crates/apps/qip-api/src/stream.rs` finds nothing — so there is nothing to
 * apply there today. A frame that ever carried one would need this applied in
 * `src/app/api/stream/[channel]/route.ts` too, and this sentence is here so
 * the next reader does not assume that route is already covered.
 */

/**
 * What stands in the place of a redacted value.
 *
 * A visible marker rather than a deleted key, because a body that quietly
 * lost a field reads as a platform that stopped serving it, and this console
 * exists to keep those two apart. It is deliberately not address-shaped.
 */
export const REDACTED = "[redacted by this console's gateway]";

/** The type of a field a browser never receives the real value of. */
export type Redacted = typeof REDACTED;

/** The path segment that means "every element of this array". */
export const EVERY = "[]";

export interface WireRedaction {
  /** The route, as a path under `/api/v1`. */
  readonly route: string;
  /** The field, written the way it reads in the body. For a person. */
  readonly field: string;
  /** The walk to it. {@link EVERY} descends into every array element. */
  readonly path: readonly string[];
  /** What reaching a browser would cost. */
  readonly why: string;
}

export const WIRE_REDACTIONS: readonly WireRedaction[] = [
  {
    route: "/mesh",
    field: "cells[].address",
    path: ["cells", EVERY, "address"],
    why:
      "a cell's base URL on the mesh transport, from QIP_MESH_PEER — an internal endpoint, and the " +
      "cell's identity on that transport rather than a label for it. No page in this console renders " +
      "it, so nothing here loses a fact by its removal.",
  },
  {
    route: "/system/status",
    field: "mesh.cells[].address",
    path: ["mesh", "cells", EVERY, "address"],
    why:
      "the same field: SystemStatus embeds the mesh status whole, so a route nobody thinks of as the " +
      "mesh route serves every cell address too. Redacting /mesh alone would have moved the leak " +
      "rather than closed it.",
  },
] as const;

export interface WireDisclosure {
  /** The route, as a path under `/api/v1`. */
  readonly route: string;
  /** The fields it carries that a reader would expect this console to withhold. */
  readonly fields: readonly string[];
  /** The least platform role the route is served to. */
  readonly role: string;
  /** Why the gateway does not remove them. */
  readonly why: string;
  /** The change that would actually close it, and whose it is. */
  readonly platform_fix: string;
}

export const WIRE_DISCLOSURES: readonly WireDisclosure[] = [
  {
    route: "/registrations",
    // Four fields until the platform split the route; one now. `secret_slot`,
    // `secret_command` and each companion command moved to
    // GET /api/v1/registrations/slots at Role::Operator, and the `secret` on a
    // registered standing went with them — so a viewer credential no longer
    // receives any of them, and this entry no longer claims it does. The list
    // shrank because the platform changed, which is the outcome the previous
    // `platform_fix` asked for; leaving the old four here would have told a
    // reader their browser holds material it has not been sent since.
    fields: ["terms"],
    role: "viewer",
    why:
      "the venue's own terms reference — a licence identifier or a public URL off the catalogue. " +
      "/compliance renders it per source because showing which licence each source is read under is " +
      "that page's whole job, and /data-sources/registrations links it as the document an operator " +
      "must read before approving. It says nothing about this deployment, which is why the platform " +
      "keeps it on the viewer's list; it is named here because this health page promises to render no " +
      "venue URL and a venue URL is nonetheless in the body behind it.",
    platform_fix:
      "None outstanding, and that is the honest entry rather than an empty string. The fix this row " +
      "used to ask for has been made: GET /api/v1/registrations answers SourceStandingView, with no " +
      "credential slot and no Secret Manager command, and GET /api/v1/registrations/slots carries " +
      "those at Role::Operator (registration_views.rs, ROUTES-REGISTRATIONS.md). `terms` is a public " +
      "licence reference the compliance surface exists to show, so withholding it would remove a fact " +
      "a page is accountable for and hide nothing from anyone.",
  },
] as const;

/** What {@link redactBody} did to one response. */
export interface Redaction {
  /** The body to send on. Byte-for-byte the original unless a field was replaced. */
  readonly body: string;
  /** The declared fields actually replaced, for the `x-qip-redacted` header. */
  readonly fields: readonly string[];
}

/**
 * Apply the declared redactions for one route to one response body.
 *
 * Conservative on purpose. A body this module cannot parse, a content type
 * that is not JSON, a route with nothing declared, and a body where the
 * declared field is absent all come back untouched and unreported: a gateway
 * that reserialised every answer would change key order and whitespace on
 * routes nobody asked it to touch, and "passed through unmodified" would stop
 * being checkable. Only a body that really carried a declared field is rebuilt,
 * and then the header says which one.
 */
export function redactBody(
  route: string,
  body: string,
  contentType: string | null,
): Redaction {
  const declared = WIRE_REDACTIONS.filter((entry) => entry.route === route);
  if (declared.length === 0) return { body, fields: [] };
  if (contentType !== null && !contentType.includes("json")) return { body, fields: [] };

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    // Not this gateway's business to fix. The platform's answer goes on as it
    // came, and the page reports whatever it can make of it.
    return { body, fields: [] };
  }

  const fields: string[] = [];
  for (const entry of declared) {
    if (replaceAt(parsed, entry.path, REDACTED) > 0) fields.push(entry.field);
  }
  if (fields.length === 0) return { body, fields: [] };
  return { body: JSON.stringify(parsed), fields };
}

/**
 * Replace every value the walk reaches, and report how many.
 *
 * An absent key and a null value are both left alone and both count zero: the
 * marker means "there was something here and this console removed it", so
 * writing it over nothing would report a redaction that never happened.
 */
function replaceAt(node: unknown, path: readonly string[], marker: string): number {
  const head = path[0];
  if (head === undefined) return 0;
  const rest = path.slice(1);

  if (head === EVERY) {
    if (!Array.isArray(node)) return 0;
    let count = 0;
    for (const element of node) count += replaceAt(element, rest, marker);
    return count;
  }

  if (typeof node !== "object" || node === null || Array.isArray(node)) return 0;
  const record = node as Record<string, unknown>;
  if (rest.length > 0) return replaceAt(record[head], rest, marker);

  const current = record[head];
  if (current === undefined || current === null) return 0;
  record[head] = marker;
  return 1;
}
