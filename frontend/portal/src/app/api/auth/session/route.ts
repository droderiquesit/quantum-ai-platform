import { NextResponse, type NextRequest } from "next/server";
import { noStore, sessionFrom } from "@/lib/server/auth-http";
import { publicSession } from "@/lib/server/identity";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(request: NextRequest): Promise<NextResponse> {
  const claims = sessionFrom(request);
  const session = claims ? publicSession(claims) : null;
  return NextResponse.json(session ?? { status: "unauthenticated" }, { headers: noStore });
}
