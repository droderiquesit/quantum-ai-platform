import { NextResponse, type NextRequest } from "next/server";
import { noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { forgotPassword } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

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
