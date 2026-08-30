import { NextResponse, type NextRequest } from "next/server";
import { clearSession, requireCsrf, sessionIdFrom } from "@/lib/server/auth-http";
import { identityStore } from "@/lib/server/identity-store";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * Sign-out revokes server-side, then clears the cookie. Clearing only the
 * cookie would leave a live session an attacker with the old value could
 * still replay — "logged out" must mean the server no longer honours it.
 */
export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;

  const sessionId = sessionIdFrom(request);
  if (sessionId) identityStore.deleteSession(sessionId);

  const response = new NextResponse(null, { status: 204, headers: { "cache-control": "no-store" } });
  clearSession(response);
  return response;
}
