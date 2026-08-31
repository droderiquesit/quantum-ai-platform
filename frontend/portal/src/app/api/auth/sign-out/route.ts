import { NextResponse, type NextRequest } from "next/server";
import { clearSession, requireCsrf } from "@/lib/server/auth-http";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * Sign-out clears the cookie, and clearing the cookie is all sign-out is.
 *
 * This used to also delete a server-side record, and the comment here used to
 * say that clearing the cookie alone would leave a session an attacker could
 * replay. Both were true of a design that no longer exists: the session record
 * lived in a JSON file under `/tmp`, which on Cloud Run is per-instance and in
 * memory, so the delete removed it from whichever instance happened to serve
 * this request and from none of the others. It read as revocation and was not.
 *
 * ADR 0019 makes the sealed cookie the session outright and says plainly what
 * that costs: a copy taken from the browser stays valid until it expires. What
 * this endpoint does — end the session on this device — it now does reliably,
 * which is more than the previous arrangement managed.
 */
export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;

  const response = new NextResponse(null, { status: 204, headers: { "cache-control": "no-store" } });
  clearSession(response);
  return response;
}
