import { NextResponse, type NextRequest } from "next/server";
import { password as passwordRule, validate } from "@algorik/validation";
import { noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { resetPassword } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

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
