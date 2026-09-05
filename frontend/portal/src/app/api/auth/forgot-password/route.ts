import { NextResponse, type NextRequest } from "next/server";
import { noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { forgotPassword } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * Request a password reset code.
 *
 * @deprecated Retired under ADR 0038 (proposed, not applied). There is no
 * password to reset once passkeys are the only credential; recovery becomes a
 * second passkey or an operator-issued one-time re-enrolment code that opens
 * only the enrolment ceremony. Working until then.
 */
export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;
  const body = await readJson<{ email?: string }>(request);
  if (!body?.email) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: "Enter the email address." } },
      { status: 400, headers: noStore },
    );
  }
  // Always 200: whether the account exists is never revealed here.
  const { devCode } = await forgotPassword(body.email);
  return NextResponse.json({ ok: true, ...(devCode ? { devCode } : {}) }, { headers: noStore });
}
