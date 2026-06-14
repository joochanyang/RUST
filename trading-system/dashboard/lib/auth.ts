import { createHmac, randomBytes, timingSafeEqual } from "crypto";
import { cookies } from "next/headers";
import { NextResponse } from "next/server";

const sessionCookieName = "trading_dashboard_session";
const sessionMaxAgeSeconds = 12 * 60 * 60;

type CookieOptions = Parameters<NextResponse["cookies"]["set"]>[2];

export function isDashboardAuthConfigured() {
  return Boolean(process.env.DASHBOARD_PASSWORD);
}

export async function isDashboardAuthenticated() {
  if (!isDashboardAuthConfigured()) {
    return true;
  }

  const cookieStore = await cookies();
  const session = cookieStore.get(sessionCookieName)?.value;
  return Boolean(session && verifySession(session));
}

export async function requireDashboardAuth() {
  if (await isDashboardAuthenticated()) {
    return null;
  }

  return NextResponse.json({ error: "Dashboard authentication required" }, { status: 401 });
}

export function validateDashboardPassword(password: string) {
  const expected = process.env.DASHBOARD_PASSWORD;
  return Boolean(expected && safeEqual(password, expected));
}

export function setDashboardSession(response: NextResponse) {
  response.cookies.set(sessionCookieName, createSession(), cookieOptions(sessionMaxAgeSeconds));
}

export function clearDashboardSession(response: NextResponse) {
  response.cookies.set(sessionCookieName, "", cookieOptions(0));
}

function createSession() {
  const expiresAt = Math.floor(Date.now() / 1000) + sessionMaxAgeSeconds;
  const nonce = randomBytes(16).toString("hex");
  const payload = `${expiresAt}.${nonce}`;
  return `${payload}.${sign(payload)}`;
}

function verifySession(session: string) {
  const [expiresAtRaw, nonce, signature] = session.split(".");
  if (!expiresAtRaw || !nonce || !signature) {
    return false;
  }

  const expiresAt = Number(expiresAtRaw);
  if (!Number.isInteger(expiresAt) || expiresAt <= Math.floor(Date.now() / 1000)) {
    return false;
  }

  const payload = `${expiresAtRaw}.${nonce}`;
  return safeEqual(signature, sign(payload));
}

function sign(payload: string) {
  return createHmac("sha256", sessionSecret()).update(payload).digest("hex");
}

function sessionSecret() {
  return process.env.DASHBOARD_SESSION_SECRET ?? process.env.DASHBOARD_PASSWORD ?? "local-dashboard-session";
}

function safeEqual(left: string, right: string) {
  const leftBuffer = Buffer.from(left);
  const rightBuffer = Buffer.from(right);

  if (leftBuffer.length !== rightBuffer.length) {
    return false;
  }

  return timingSafeEqual(leftBuffer, rightBuffer);
}

function cookieOptions(maxAge: number): CookieOptions {
  return {
    httpOnly: true,
    sameSite: "strict",
    secure: process.env.NODE_ENV === "production",
    path: "/",
    maxAge
  };
}
