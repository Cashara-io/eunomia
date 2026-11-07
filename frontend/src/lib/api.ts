// Same-origin by default: next.config.ts proxies /api/* to the backend, so
// the browser never needs to know the backend's host (works from any device
// or hostname, no CORS). NEXT_PUBLIC_API_URL only overrides that.
export const API_URL = process.env.NEXT_PUBLIC_API_URL || "";
export const apiOrigin = () => API_URL || window.location.origin;

// Session is an httpOnly JWT cookie: every request needs `credentials: "include"`.
export class ApiError extends Error {
  status: number;
  code: string;
  detail: string;
  traceId?: string;
  constructor(status: number, code: string, detail: string, traceId?: string) {
    super(detail);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.detail = detail;
    this.traceId = traceId;
  }
}

const hex = (n: number) =>
  Array.from(crypto.getRandomValues(new Uint8Array(n)), (b) => b.toString(16).padStart(2, "0")).join("");

// W3C traceparent (version 00, sampled) so the backend joins the browser's trace.
export const traceparent = () => `00-${hex(16)}-${hex(8)}-01`;

// Error bodies are RFC 9457 problem+json: detail, code, trace_id. Same fields `toApiError` reads for the generated client.
async function apiErrorFromResponse(res: Response, path: string): Promise<ApiError> {
  let detail = `${res.status} ${path}`;
  let code = `http.${res.status}`;
  let traceId = res.headers.get("x-trace-id") ?? undefined;
  try {
    const body = await res.json();
    if (typeof body?.detail === "string") detail = body.detail;
    if (typeof body?.code === "string") code = body.code;
    if (typeof body?.trace_id === "string") traceId = body.trace_id;
  } catch {
    // non-JSON error body, fall back to the status line above
  }
  return new ApiError(res.status, code, detail, traceId);
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/** Saves a response body as a file named `name`. */
async function saveResponse(res: Response, path: string, name: string): Promise<void> {
  if (!res.ok) throw await apiErrorFromResponse(res, path);
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

/** Downloads a document's original bytes (`kind: "download"`) or its export manifest with vectors (`"export"`). */
export async function downloadDocumentFile(id: string, filename: string, kind: "download" | "export"): Promise<void> {
  const path = `/api/documents/${encodeURIComponent(id)}/${kind}`;
  const res = await fetch(`${API_URL}${path}`, { credentials: "include", headers: { traceparent: traceparent() } });
  await saveResponse(res, path, kind === "download" ? filename : `${filename}.export.json`);
}

/**
 * Uploads one file as the raw request body (XMLHttpRequest, for upload progress). The browser's type
 * goes in the query when it has one; without it the server goes by the file extension.
 */
export function uploadDocumentFile(file: File, onProgress: (fraction: number) => void): Promise<{ id: string; status: string }> {
  const params = new URLSearchParams({ filename: file.name });
  if (file.type) params.set("content_type", file.type);
  const path = `/api/documents?${params}`;
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `${API_URL}${path}`);
    xhr.withCredentials = true;
    xhr.setRequestHeader("traceparent", traceparent());
    xhr.setRequestHeader("Content-Type", file.type || "application/octet-stream");
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) onProgress(e.loaded / e.total);
    };
    xhr.onload = () => {
      let body: { id?: string; status?: string; detail?: string; code?: string; trace_id?: string } = {};
      try {
        body = JSON.parse(xhr.responseText);
      } catch {
        // a proxy's plain-text error (a body too large for it, say)
      }
      if (xhr.status >= 200 && xhr.status < 300 && body.id) {
        resolve({ id: body.id, status: body.status ?? "indexing" });
        return;
      }
      const tooLarge = xhr.status === 413 && !body.detail;
      reject(
        new ApiError(
          xhr.status,
          body.code ?? (tooLarge ? "document.too_large" : `http.${xhr.status}`),
          body.detail ?? (tooLarge ? "That file is larger than the server accepts." : `${xhr.status} /api/documents`),
          body.trace_id ?? xhr.getResponseHeader("x-trace-id") ?? undefined,
        ),
      );
    };
    xhr.onerror = () => reject(new Error("The upload did not reach the server. Check your connection and try again."));
    xhr.send(file);
  });
}

export async function downloadExport(): Promise<void> {
  const res = await fetch(`${API_URL}/api/export`, { credentials: "include", headers: { traceparent: traceparent() } });
  if (!res.ok) throw await apiErrorFromResponse(res, "/api/export");
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = "eunomia-export.json";
  a.click();
  URL.revokeObjectURL(url);
}

// ---------------------------------------------------------------------------
// chat
// ---------------------------------------------------------------------------

// One parsed Server-Sent Event from `POST /api/chat/threads/{id}` -- see
// the backend chat module for the exact event shapes.
export type ChatStreamEvent =
  | { type: "text"; delta: string }
  // `id`: the model's id for the call; its tool_result repeats it
  | { type: "tool_call"; id?: string; name: string }
  // `view`: what a `show_view` call drew (components/chat/views); no other tool's result carries one
  | { type: "tool_result"; id?: string; name: string; view?: unknown }
  | { type: "done"; reply: string; tool_calls_made: string[] }
  | { type: "error"; message: string; code?: string; trace_id?: string };

export const chat = {
  // Streams a reply for `threadId`, calling `onEvent` for every parsed SSE
  // event as it arrives. Not built on the generated client/EventSource: EventSource
  // can't send a POST body, and this needs to parse a `ReadableStream`
  // chunk-by-chunk rather than wait for the whole response.
  send: async (
    threadId: string,
    message: string,
    onEvent: (event: ChatStreamEvent) => void,
    signal?: AbortSignal
  ): Promise<void> => {
    const res = await fetch(`${API_URL}/api/chat/threads/${encodeURIComponent(threadId)}`, {
      method: "POST",
      signal,
      credentials: "include",
      headers: { "Content-Type": "application/json", traceparent: traceparent() },
      body: JSON.stringify({ message }),
    });
    if (!res.ok || !res.body) {
      throw await apiErrorFromResponse(res, `/api/chat/threads/${threadId}`);
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split("\n\n");
      buffer = lines.pop() ?? "";
      for (const line of lines) {
        if (!line.startsWith("data: ")) continue;
        onEvent(JSON.parse(line.slice("data: ".length)) as ChatStreamEvent);
      }
    }
  },
};
