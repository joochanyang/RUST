import { cookies } from "next/headers";
import { NextResponse } from "next/server";

const csrfCookieName = "trading_dashboard_csrf";

export async function GET() {
  const cookieStore = await cookies();
  const existing = cookieStore.get(csrfCookieName)?.value;
  const token = existing ?? crypto.randomUUID();

  const response = NextResponse.json({ token });
  if (!existing) {
    response.cookies.set(csrfCookieName, token, {
      httpOnly: false,
      sameSite: "strict",
      secure: process.env.NODE_ENV === "production",
      path: "/"
    });
  }

  return response;
}
