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
  const deadline = new AbortController();
  const signal = AbortSignal.any([request.signal, deadline.signal]);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let tooLarge = false;
  let body: ReadableStream<Uint8Array> | null = request.body;
  try {
    if (body !== null) {
      const authHeaders = new Headers(headers);
      for (const name of ["content-length", "content-type", "content-encoding", "mcp-session-id", "mcp-protocol-version"]) authHeaders.delete(name);
      const mcp = url.pathname === "/mcp";
      // HEAD authenticates with the actual route and reports the scope-bound body limit.
      const auth = await fetch(`${backend}${mcp ? "/mcp" : "/api/documents"}`, {
        method: "HEAD",
        headers: authHeaders,
        cache: "no-store",
        redirect: "manual",
        signal: AbortSignal.any([request.signal, AbortSignal.timeout(5000)]),
      });
      if (auth.status !== 200) {
        void body.cancel().catch(() => {});
        if (auth.status >= 400) {
          return new Response(auth.body, { status: auth.status, headers: auth.headers });
        }
        await auth.body?.cancel();
        return Response.json({ detail: "Backend unavailable" }, { status: 503 });
      }
      const limit = Number(auth.headers.get("x-eunomia-body-limit"));
      const uploadAllowed = auth.headers.get("x-eunomia-upload-allowed") === "1";
      await auth.body?.cancel();
      if (!Number.isSafeInteger(limit) || limit <= 0) {
        void body.cancel().catch(() => {});
        return Response.json({ detail: "Backend unavailable" }, { status: 503 });
      }
      if (Number(headers.get("content-length")) > limit) {
        void body.cancel().catch(() => {});
        return Response.json({ detail: "Request exceeds the token's body limit" }, { status: uploadAllowed ? 413 : 403 });
      }
      // The optional setting can shorten the deadline, never extend the two-minute ceiling.
      const configured = Number(process.env.EUNOMIA_UPLOAD_BODY_TIMEOUT_MS);
      const timeout = configured > 0 && Number.isFinite(configured) ? Math.min(configured, 120_000) : 120_000;
      timer = setTimeout(() => deadline.abort(), timeout);
      let bytes = 0;
      body = body.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
        transform(chunk, controller) {
          bytes += chunk.byteLength;
          if (bytes > limit) {
            tooLarge = true;
            throw new Error("Request exceeds the token's body limit");
          }
          controller.enqueue(chunk);
        },
        flush() { clearTimeout(timer); },
      }), { signal });
    }
    const response = await fetch(`${backend}${url.pathname}${url.search}`, {
      method: request.method,
      headers,
      body: request.method === "GET" || request.method === "HEAD" ? undefined : body,
      ...{ duplex: "half" },
      cache: "no-store",
      redirect: "manual",
      signal,
    });
    const responseHeaders = new Headers(response.headers);
    responseHeaders.delete("content-encoding");
    responseHeaders.delete("content-length");
    responseHeaders.delete("transfer-encoding");
    responseHeaders.delete("connection");
    return new Response(response.body, { status: response.status, headers: responseHeaders });
  } catch {
    if (tooLarge) return Response.json({ detail: "Request body too large" }, { status: 413 });
    if (deadline.signal.aborted) return Response.json({ detail: "Upload body timed out" }, { status: 408 });
    return Response.json({ detail: "Backend unavailable" }, { status: 503 });
  } finally {
    clearTimeout(timer);
  }
}
