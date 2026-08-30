import { NextResponse } from "next/server";
import { attachCsrf, noStore } from "@/lib/server/auth-http";
import { newCsrfToken } from "@/lib/server/session";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * Issues the CSRF pair: the token in the body for the page to echo in a
 * header, and the same token in a cookie the page cannot forge cross-origin.
 */
export async function GET(): Promise<NextResponse> {
  const token = newCsrfToken();
  const response = NextResponse.json({ token }, { headers: noStore });
  attachCsrf(response, token);
  return response;
}
