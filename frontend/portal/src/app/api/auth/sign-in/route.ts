import { NextResponse, type NextRequest } from "next/server";
import { attachSession, deviceFrom, noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { signIn } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;

  const body = await readJson<{ email?: string; password?: string }>(request);
  if (!body?.email || !body?.password) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: "Enter an email address and a password." } },
      { status: 400, headers: noStore },
    );
  }

  const result = await signIn(body.email, body.password, deviceFrom(request));
  if (!result.ok) {
    return NextResponse.json(
      { ok: false, failure: result.failure },
      { status: result.status, headers: noStore },
    );
  }

  const response = NextResponse.json({ ok: true }, { headers: noStore });
  attachSession(response, result.value);
  return response;
}
