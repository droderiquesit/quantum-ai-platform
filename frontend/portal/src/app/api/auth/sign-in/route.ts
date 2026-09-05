import { NextResponse, type NextRequest } from "next/server";
import { attachSession, deviceFrom, noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { signIn } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * Password sign-in.
 *
 * @deprecated Passkeys are the recorded destination (ADR 0038, proposed, not
 * applied): an account will have no password, and this route is retired —
 * `410 Gone`, naming the sign-in page — once a passkey can be enrolled and
 * asserted on a deployment. Until then it is the working credential path and
 * must not be weakened; the note exists so nobody extends it as the standing
 * method.
 */
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
