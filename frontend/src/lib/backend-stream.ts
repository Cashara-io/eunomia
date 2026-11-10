const backend = process.env.BACKEND_INTERNAL_URL || "http://localhost:8001";

function upstreamHeaders(request: Request) {
  const headers = new Headers(request.headers);
  for (const name of ["host", "connection", "transfer-encoding", "expect"]) headers.delete(name);
  headers.set("accept-encoding", "identity");
  if (process.env.FRONTEND_TRUST_FORWARDED !== "1") {
    for (const name of ["x-forwarded-for", "x-forwarded-proto", "x-real-ip"]) headers.delete(name);
  }
  return headers;
}

// Route handlers receive the original stream, without Next proxy's in-memory clone.
// Only the fixed internal backend is contacted. No request-controlled host or redirect is followed.
export async function streamBackend(request: Request) {
  const url = new URL(request.url);
  const headers = upstreamHeaders(request);
  try {
    if (request.body !== null) {
      const authHeaders = new Headers(headers);
      for (const name of ["content-length", "content-type", "content-encoding"]) authHeaders.delete(name);
      // OAuth access tokens are accepted only at /mcp. A fixed, tiny ping uses that route's
      // real authentication rules without reading any bytes of the incoming request.
      const mcp = url.pathname === "/mcp";
      authHeaders.delete("mcp-session-id");
      authHeaders.delete("mcp-protocol-version");
      if (mcp) authHeaders.set("content-type", "application/json");
      const auth = await fetch(`${backend}${mcp ? "/mcp" : "/api/auth/me"}`, {
        method: mcp ? "POST" : "GET",
        body: mcp ? '{"jsonrpc":"2.0","id":"proxy-auth","method":"ping"}' : undefined,
        headers: authHeaders,
        cache: "no-store",
        redirect: "manual",
        signal: AbortSignal.any([request.signal, AbortSignal.timeout(5000)]),
      });
      if (auth.status !== 200) {
        if (auth.status >= 400) {
          return new Response(auth.body, { status: auth.status, headers: auth.headers });
        }
        await auth.body?.cancel();
        return Response.json({ detail: "Backend unavailable" }, { status: 503 });
      }
      await auth.body?.cancel();
    }
    const response = await fetch(`${backend}${url.pathname}${url.search}`, {
      method: request.method,
      headers,
      body: request.method === "GET" || request.method === "HEAD" ? undefined : request.body,
      // Node fetch requires this for an unbuffered request body.
      ...{ duplex: "half" },
      cache: "no-store",
      redirect: "manual",
      signal: request.signal,
    });
    const responseHeaders = new Headers(response.headers);
    // Fetch decodes these encodings, so their original lengths no longer describe the stream.
    responseHeaders.delete("content-encoding");
    responseHeaders.delete("content-length");
    responseHeaders.delete("transfer-encoding");
    responseHeaders.delete("connection");
    return new Response(response.body, { status: response.status, headers: responseHeaders });
  } catch {
    return Response.json({ detail: "Backend unavailable" }, { status: 503 });
  }
}
