import { NextRequest, NextResponse } from "next/server";
import {
  clearDashboardSession,
  isDashboardAuthConfigured,
  isDashboardAuthenticated,
  setDashboardSession,
  validateDashboardPassword
} from "../../../lib/auth";

export async function GET() {
  return NextResponse.json({
    authRequired: isDashboardAuthConfigured(),
    authenticated: await isDashboardAuthenticated()
  });
}

export async function POST(request: NextRequest) {
  if (!isDashboardAuthConfigured()) {
    return NextResponse.json({ authenticated: true });
  }

  const payload = await request.json().catch(() => null) as { password?: string } | null;
  if (!payload?.password || !validateDashboardPassword(payload.password)) {
    return NextResponse.json({ error: "Invalid dashboard password" }, { status: 401 });
  }

  const response = NextResponse.json({ authenticated: true });
  setDashboardSession(response);
  return response;
}

export async function DELETE() {
  const response = NextResponse.json({ authenticated: false });
  clearDashboardSession(response);
  return response;
}
