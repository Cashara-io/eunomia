import { NextResponse, type NextRequest } from "next/server";

// The Next rewrites to the backend forward X-Forwarded-* exactly as the client sent them, and
// Next does not expose the TCP peer to proxy code, so the true peer cannot be written here.
// Unless a trusted reverse proxy (Caddy, nginx) sits in front and FRONTEND_TRUST_FORWARDED=1,
// drop client-supplied forwarding headers so a client reaching :3000 directly cannot spoof its
// address or scheme. See docs/deployment.md.
export function proxy(request: NextRequest) {
  if (process.env.FRONTEND_TRUST_FORWARDED === "1") return NextResponse.next();
  const headers = new Headers(request.headers);
  for (const h of ["x-forwarded-for", "x-forwarded-proto", "x-real-ip"]) headers.delete(h);
  return NextResponse.next({ request: { headers } });
}

export const config = {
  // These exact routes have streaming handlers. Matching them here would clone their body
  // before the handler can authenticate, and would truncate authenticated uploads at 1 MiB.
  matcher: ["/api/((?!documents/?$).*)", "/healthz", "/readyz", "/.well-known/:path*", "/oauth/:path*"],
};
