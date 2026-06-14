import { cookies } from "next/headers";
import { NextRequest, NextResponse } from "next/server";
import { requireDashboardAuth } from "../../../lib/auth";

const csrfCookieName = "trading_dashboard_csrf";

type RouteContext = {
  params: Promise<{ path: string[] }>;
};

export async function GET(request: NextRequest, context: RouteContext) {
  const authError = await requireDashboardAuth();
  if (authError) {
    return authError;
  }

  return proxyToTradingApi(request, context);
}

export async function POST(request: NextRequest, context: RouteContext) {
  const authError = await requireDashboardAuth();
  if (authError) {
    return authError;
  }

  const csrfError = await validateCsrf(request);
  if (csrfError) {
    return csrfError;
  }

  return proxyToTradingApi(request, context);
}

async function proxyToTradingApi(request: NextRequest, context: RouteContext) {
  const { path } = await context.params;
  const upstreamUrl = buildUpstreamUrl(path, request.nextUrl.search);
  const headers = new Headers();
  const contentType = request.headers.get("content-type");
  const controlToken = process.env.DASHBOARD_CONTROL_TOKEN;

  if (contentType) {
    headers.set("content-type", contentType);
  }

  if (controlToken) {
    headers.set("x-dashboard-control-token", controlToken);
  }

  const response = await fetch(upstreamUrl, {
    method: request.method,
    headers,
    body: request.method === "GET" || request.method === "HEAD" ? undefined : await request.text(),
    cache: "no-store"
  });

  return new NextResponse(response.body, {
    status: response.status,
    headers: responseHeaders(response)
  });
}

function buildUpstreamUrl(path: string[], search: string) {
  const baseUrl =
    process.env.API_BASE_URL ??
    process.env.NEXT_PUBLIC_API_BASE_URL ??
    "http://127.0.0.1:8080";
  const normalizedBase = baseUrl.replace(/\/$/, "");
  const normalizedPath = path.map(encodeURIComponent).join("/");

  return `${normalizedBase}/api/${normalizedPath}${search}`;
}

async function validateCsrf(request: NextRequest) {
  const cookieStore = await cookies();
  const cookieToken = cookieStore.get(csrfCookieName)?.value;
  const headerToken = request.headers.get("x-csrf-token");

  if (!cookieToken || !headerToken || cookieToken !== headerToken) {
    return NextResponse.json({ error: "CSRF token is invalid" }, { status: 403 });
  }

  return null;
}

function responseHeaders(response: Response) {
  const headers = new Headers();
  const contentType = response.headers.get("content-type");

  if (contentType) {
    headers.set("content-type", contentType);
  }

  return headers;
}
