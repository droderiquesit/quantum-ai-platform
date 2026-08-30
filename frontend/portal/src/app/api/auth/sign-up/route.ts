import { NextResponse, type NextRequest } from "next/server";
import { email as emailRule, password as passwordRule, validate } from "@algorik/validation";
import { noStore, readJson, requireCsrf } from "@/lib/server/auth-http";
import { signUp, type SignUpInput } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const ACCOUNT_TYPES = new Set(["individual", "institutional", "partner", "developer"]);

export async function POST(request: NextRequest): Promise<NextResponse> {
  const refused = requireCsrf(request);
  if (refused) return refused;

  const body = await readJson<SignUpInput>(request);
  if (!body || typeof body.email !== "string" || typeof body.password !== "string") {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: "The request body was not understood." } },
      { status: 400, headers: noStore },
    );
  }
  // Server-side validation is the one that counts; the page's copy of these
  // rules is a convenience an attacker never sees.
  const checked = validate(
    { email: body.email, password: body.password },
    { email: [emailRule()], password: [passwordRule()] },
  );
  if (!checked.ok) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: checked.errors[0]?.message ?? "Invalid input." } },
      { status: 400, headers: noStore },
    );
  }
  if (!ACCOUNT_TYPES.has(body.accountType)) {
    return NextResponse.json(
      { ok: false, failure: { code: "invalid_credentials", message: "Choose an account type." } },
      { status: 400, headers: noStore },
    );
  }

  const result = await signUp(body);
  if (!result.ok) {
    return NextResponse.json({ ok: false, failure: result.failure }, { status: result.status, headers: noStore });
  }
  return NextResponse.json(
    { ok: true, next: "verify-email", ...(result.value.devCode ? { devCode: result.value.devCode } : {}) },
    { headers: noStore },
  );
}
