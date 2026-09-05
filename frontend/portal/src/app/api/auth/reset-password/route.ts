import { NextResponse, type NextRequest } from "next/server";
import { password as passwordRule, validate } from "@algorik/validation";
import { noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { resetPassword } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * Redeem a reset code and set a new password.
 *
 * @deprecated Retired under ADR 0038 (proposed, not applied), together with
 * `forgot-password`: a password-free account has nothing for this route to
 * set. The one-time-code discipline it uses — HMAC-stored, spend-down
 * attempts, short life — is what the re-enrolment code reuses. Working until
 * then.
 */
export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;
  const body = await readJson<{ email?: string; code?: string; password?: string }>(request);
  if (!body?.email || !body?.code || !body?.password) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: "Enter the email, the code, and a new password." } },
      { status: 400, headers: noStore },
    );
  }
  const checked = validate({ password: body.password }, { password: [passwordRule()] });
  if (!checked.ok) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: checked.errors[0]?.message ?? "Invalid password." } },
      { status: 400, headers: noStore },
    );
  }
  const result = await resetPassword(body.email, body.code, body.password);
  return result.ok
    ? NextResponse.json({ ok: true }, { headers: noStore })
    : NextResponse.json({ ok: false, failure: result.failure }, { status: result.status, headers: noStore });
}
