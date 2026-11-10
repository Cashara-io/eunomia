import { Agent } from "undici";
import { UploadBudget, uploadPolicy } from "./upload-budget";

// The body budget controls uploads. Undici's default five-minute header deadline
// would otherwise expire while a legitimate body is still being sent.
// Bun ignores dispatcher, so disable its fetch idle deadline explicitly too.
const dispatcher = new Agent({ headersTimeout: 0, bodyTimeout: 0, connectTimeout: 5000 });

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
      const setting = (name: string, fallback: number) => {
        const value = Number(process.env[name]);
        return Number.isFinite(value) && value > 0 ? Math.min(value, fallback) : fallback;
      };
      const policy = {
        ...uploadPolicy,
        graceMs: setting("EUNOMIA_UPLOAD_GRACE_MS", uploadPolicy.graceMs),
        windowMs: setting("EUNOMIA_UPLOAD_WINDOW_MS", uploadPolicy.windowMs),
      };
      const budget = new UploadBudget(limit, policy);
      timer = setInterval(() => { if (budget.expired()) deadline.abort(); }, Math.min(1000, policy.windowMs / 10));
      let bytes = 0;
      body = body.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
        transform(chunk, controller) {
          budget.record(chunk.byteLength);
          if (budget.expired()) { deadline.abort(); throw new Error("Upload body timed out"); }
          bytes += chunk.byteLength;
          if (bytes > limit) {
            tooLarge = true;
            throw new Error("Request exceeds the token's body limit");
          }
          controller.enqueue(chunk);
        },
        flush() { clearInterval(timer); },
      }), { signal });
    }
    const response = await fetch(`${backend}${url.pathname}${url.search}`, {
      ...{ dispatcher, timeout: false },
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
    clearInterval(timer);
  }
}
