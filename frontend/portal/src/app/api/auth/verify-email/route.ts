import { NextResponse, type NextRequest } from "next/server";
import { noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { verifyEmail } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;
  const body = await readJson<{ email?: string; code?: string }>(request);
  if (!body?.email || !body?.code) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: "Enter the email address and the code." } },
      { status: 400, headers: noStore },
    );
  }
  const result = await verifyEmail(body.email, body.code);
  return result.ok
    ? NextResponse.json({ ok: true }, { headers: noStore })
    : NextResponse.json({ ok: false, failure: result.failure }, { status: result.status, headers: noStore });
}
