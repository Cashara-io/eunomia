"use client";

import { ArrowClockwiseIcon, DownloadSimpleIcon, FileCodeIcon, FileTextIcon, FilesIcon, TrashIcon, UploadSimpleIcon, XIcon } from "@phosphor-icons/react/ssr";
import { Suspense, useEffect, useRef, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import PageHeader from "@/components/ui/PageHeader";
import { EmptyState } from "@/components/Shapes";
import StatusLabel, { type StatusTone } from "@/components/ui/StatusLabel";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import CopyButton from "@/components/bits/CopyButton";
import { downloadDocumentFile, uploadDocumentFile } from "@/lib/api";
import type { DocumentOut } from "@/lib/gen";
import { useDeleteDocument, useDocument, useDocuments, useRefreshDocuments, useReindexDocument } from "@/lib/queries/documents";

const ICON = { size: 18, weight: "duotone" } as const;
const ACCEPT = ".txt,.text,.md,.markdown,.json,.csv,.pdf,text/plain,text/markdown,application/json,text/csv,application/pdf";
const EXTENSIONS = ["txt", "text", "log", "md", "markdown", "json", "csv", "pdf"];
const TYPE_LABEL: Record<string, string> = {
  "text/plain": "Text",
  "text/markdown": "Markdown",
  "application/json": "JSON",
  "text/csv": "CSV",
  "application/pdf": "PDF",
};

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function ago(iso: string): string {
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  return days < 30 ? `${days}d ago` : new Date(iso).toLocaleDateString();
}

const STATUS: Record<string, { tone: StatusTone; label: string }> = {
  ready: { tone: "good", label: "Ready" },
  indexing: { tone: "warning", label: "Indexing" },
  failed: { tone: "critical", label: "Failed" },
  deleted: { tone: "neutral", label: "Deleting" },
};

function Status({ status }: { status: string }) {
  const s = STATUS[status] ?? STATUS.failed;
  return (
    <StatusLabel tone={s.tone} className="whitespace-nowrap">
      {s.label}
    </StatusLabel>
  );
}

// ---- uploads -------------------------------------------------------------------------------

type UploadRow = { key: string; name: string; progress: number; error: Failure | null };

function useUploads(maxBytes: number | null, onUploaded: (id: string) => void) {
  const [rows, setRows] = useState<UploadRow[]>([]);
  const refresh = useRefreshDocuments();
  const patch = (key: string, p: Partial<UploadRow>) => setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...p } : r)));

  function add(files: FileList | File[]) {
    for (const file of Array.from(files)) {
      const key = `${file.name}-${file.size}-${Math.random().toString(36).slice(2)}`;
      const ext = file.name.includes(".") ? file.name.split(".").pop()!.toLowerCase() : "";
      // checked here first so a wrong file fails at once; the server checks again
      const early: Failure | null =
        maxBytes !== null && file.size > maxBytes
          ? { message: `${file.name} is ${size(file.size)}; the limit is ${size(maxBytes)}.`, code: "document.too_large" }
          : !EXTENSIONS.includes(ext)
            ? { message: `${file.name} is not a supported type. Upload text, Markdown, JSON, CSV or a text-based PDF.`, code: "document.unsupported_type" }
            : file.size === 0
              ? { message: `${file.name} is empty.` }
              : null;
      setRows((rs) => [...rs, { key, name: file.name, progress: 0, error: early }]);
      if (early) continue;
      uploadDocumentFile(file, (f) => patch(key, { progress: f }))
        .then(async (doc) => {
          setRows((rs) => rs.filter((r) => r.key !== key));
          await refresh();
          onUploaded(doc.id);
        })
        .catch((e) => patch(key, { error: failure(e, `Could not upload ${file.name}. Try again.`) }));
    }
  }

  const dismiss = (key: string) => setRows((rs) => rs.filter((r) => r.key !== key));
  return { rows, add, dismiss };
}

function DropZone({ maxBytes, disabled, onFiles }: { maxBytes: number | null; disabled: boolean; onFiles: (f: FileList) => void }) {
  const input = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);
  return (
    <div
      className="flex flex-col items-center justify-center gap-3 px-4 py-8 text-center rounded-[12px]"
      style={{
        border: over ? "1px solid var(--ink)" : "1px dashed var(--border-field)",
        background: over ? "var(--surface-raised)" : "var(--surface)",
        transition: "background-color var(--dur-hover) ease, border-color var(--dur-hover) ease",
      }}
      onDragOver={(e) => {
        if (disabled) return;
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        if (!disabled && e.dataTransfer.files.length) onFiles(e.dataTransfer.files);
      }}
    >
      <UploadSimpleIcon weight="duotone" size={24} aria-hidden style={{ color: "var(--ink)" }} />
      <p className="text-[16px]" style={{ color: "var(--ink)" }}>
        Drop files here, or choose files
      </p>
      <p id="upload-guidance" className="label">
        Text, Markdown, JSON, CSV or a text-based PDF{maxBytes !== null ? `, up to ${size(maxBytes)} each` : ""}.
      </p>
      <button type="button" disabled={disabled} onClick={() => input.current?.click()} className="btn btn-primary">
        Choose files
      </button>
      <input
        ref={input}
        type="file"
        multiple
        accept={ACCEPT}
        className="sr-only"
        aria-label="Choose files to upload"
        aria-describedby="upload-guidance"
        data-testid="document-file-input"
        onChange={(e) => {
          if (e.target.files?.length) onFiles(e.target.files);
          e.target.value = "";
        }}
      />
    </div>
  );
}

function UploadRows({ rows, onDismiss }: { rows: UploadRow[]; onDismiss: (key: string) => void }) {
  if (rows.length === 0) return null;
  return (
    <div className="ledger hairline-rows" aria-live="polite">
      {rows.map((r) => (
        <div key={r.key} className="flex flex-col gap-2 px-4 py-3">
          <div className="flex items-center gap-3">
            <FileTextIcon {...ICON} aria-hidden style={{ color: "var(--ink-faint)" }} />
            <span className="flex-1 min-w-0 truncate text-[13px]">{r.name}</span>
            {r.error ? (
              <button type="button" onClick={() => onDismiss(r.key)} aria-label={`Dismiss ${r.name}`} className="btn btn-ghost btn-icon btn-sm">
                <XIcon {...ICON} />
              </button>
            ) : (
              <span className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                {r.progress >= 1 ? "Storing…" : `${Math.round(r.progress * 100)}%`}
              </span>
            )}
          </div>
          {r.error ? (
            <ErrorLine error={r.error} />
          ) : (
            <div
              role="progressbar"
              aria-label={`Uploading ${r.name}`}
              aria-valuenow={Math.round(r.progress * 100)}
              aria-valuemin={0}
              aria-valuemax={100}
              className="h-1.5 rounded-full overflow-hidden"
              style={{ background: "var(--surface-raised)" }}
            >
              <div className="h-full" style={{ width: `${Math.round(r.progress * 100)}%`, background: "var(--ink)", transition: "width 120ms linear" }} />
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

// ---- detail --------------------------------------------------------------------------------

function DeleteDocument({ doc, onDeleted }: { doc: DocumentOut; onDeleted: () => void }) {
  const del = useDeleteDocument();
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<Failure | null>(null);

  async function run() {
    setError(null);
    try {
      await del.mutateAsync(doc.id);
      setOpen(false);
      onDeleted();
    } catch (e) {
      setError(failure(e, `Could not delete ${doc.filename}. Try again.`));
    }
  }

  const passages = doc.chunk_count === 1 ? "its passage" : `its ${doc.chunk_count.toLocaleString()} passages`;
  return (
    <>
      <button
        type="button"
        onClick={() => {
          setError(null);
          setOpen(true);
        }}
        className="btn btn-danger btn-sm"
      >
        <TrashIcon {...ICON} aria-hidden />
        Delete
      </button>
      <ConfirmDialog
        open={open}
        title={`Delete ${doc.filename}?`}
        confirmLabel={del.isPending ? "Deleting…" : "Delete document"}
        danger
        busy={del.isPending}
        typeToConfirm={doc.filename}
        error={error?.message}
        onConfirm={run}
        onCancel={() => setOpen(false)}
      >
        <p>
          This permanently deletes the stored file, {passages} and every fact your AI apps learned from it. It disappears from search and recall at
          once. This can&apos;t be undone.
        </p>
        <p style={{ color: "var(--ink-dim)" }}>Download it first if you may need it again.</p>
      </ConfirmDialog>
    </>
  );
}

function DocumentPanel({ id, chunk, onClose }: { id: string; chunk: string | null; onClose: () => void }) {
  const query = useDocument(id);
  const reindex = useReindexDocument();
  const [busy, setBusy] = useState<"download" | "export" | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const highlighted = useRef<HTMLLIElement>(null);
  const detail = query.data;

  useEffect(() => {
    highlighted.current?.scrollIntoView({ block: "nearest" });
  }, [chunk, detail?.chunks.length]);

  async function save(kind: "download" | "export") {
    if (!detail) return;
    setBusy(kind);
    setError(null);
    try {
      await downloadDocumentFile(detail.document.id, detail.document.filename, kind);
    } catch (e) {
      setError(failure(e, kind === "download" ? "Could not download the file. Try again." : "Could not prepare the export. Try again."));
    } finally {
      setBusy(null);
    }
  }

  async function runReindex() {
    setError(null);
    try {
      await reindex.mutateAsync(id);
    } catch (e) {
      setError(failure(e, "Could not start re-indexing. Try again."));
    }
  }

  if (query.isError) {
    return (
      <div className="flex flex-col gap-2">
        <ErrorLine error={failure(query.error, "Could not load this document.", " It may have been deleted; pick another.")} />
        <button type="button" onClick={onClose} className="btn btn-sm self-start">
          Close
        </button>
      </div>
    );
  }
  if (!detail) {
    return (
      <div className="panel p-4 flex flex-col gap-3" aria-busy="true" aria-label="Loading document">
        <span className="skeleton h-5 w-40" />
        <span className="skeleton h-3 w-56" />
        <span className="skeleton h-24 w-full" />
      </div>
    );
  }
  const doc = detail.document;
  const chars = Array.from(detail.text);
  return (
    <section className="task-panel p-5 flex flex-col gap-4 min-w-0" aria-labelledby="document-title" data-testid="document-panel">
      <div className="flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <h2 id="document-title" className="text-[20px] font-medium break-words tracking-[-0.025em]">
            {doc.filename}
          </h2>
          <p className="label mt-1">
            {TYPE_LABEL[doc.media_type] ?? doc.media_type} · {size(doc.size_bytes)} · revision {doc.revision}
          </p>
        </div>
        <button type="button" onClick={onClose} aria-label="Close document" className="btn btn-ghost btn-icon btn-sm">
          <XIcon {...ICON} />
        </button>
      </div>

      <div className="flex flex-col gap-1.5 text-[13px]">
        <span data-testid="document-status">
          <Status status={doc.status} />
        </span>
        {doc.status === "failed" && doc.error && (
          <p className="text-[12.5px]" style={{ color: "var(--critical)" }}>
            {doc.error}
          </p>
        )}
        {doc.status === "indexing" && (
          <p className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
            Extracting and indexing the text. Passages appear here when it is done.
          </p>
        )}
        <span className="flex items-center gap-1 min-w-0">
          <span className="label shrink-0">SHA-256</span>
          <code className="font-mono text-[11.5px] truncate" style={{ color: "var(--ink-dim)" }}>
            {doc.sha256}
          </code>
          <CopyButton value={doc.sha256} label="Copy SHA-256" />
        </span>
      </div>

      <div className="flex flex-wrap gap-2">
        <button type="button" onClick={() => save("download")} disabled={busy !== null} aria-busy={busy === "download"} className="btn btn-sm">
          <DownloadSimpleIcon {...ICON} aria-hidden />
          {busy === "download" ? "Downloading…" : "Download"}
        </button>
        <button type="button" onClick={() => save("export")} disabled={busy !== null || doc.status !== "ready"} aria-busy={busy === "export"} className="btn btn-sm">
          <FileCodeIcon {...ICON} aria-hidden />
          {busy === "export" ? "Preparing…" : "Export"}
        </button>
        <button type="button" onClick={runReindex} disabled={reindex.isPending || doc.status === "indexing"} className="btn btn-sm">
          <ArrowClockwiseIcon {...ICON} aria-hidden />
          Re-index
        </button>
        <span className="flex-1" />
        <DeleteDocument doc={doc} onDeleted={onClose} />
      </div>
      {error && <ErrorLine error={error} />}

      <div className="flex flex-col gap-2 min-w-0">
        <h3 className="section-title flex items-center gap-2">
          Passages <span className="label font-mono">{detail.chunks.length}</span>
        </h3>
        {detail.chunks.length === 0 ? (
          <p className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
            {doc.status === "indexing" ? "Not indexed yet." : "No passages: nothing searchable came out of this file."}
          </p>
        ) : (
          <ol className="ledger hairline-rows max-h-[520px] overflow-y-auto">
            {detail.chunks.map((c) => {
              const on = c.chunk_id === chunk;
              const text = chars.slice(c.char_start, Math.min(c.char_end, c.char_start + 600)).join("");
              return (
                <li
                  key={c.chunk_id}
                  ref={on ? highlighted : undefined}
                  className={`px-3 py-2.5 flex flex-col gap-1 ${on ? "frame-selected" : ""}`}
                  style={on ? { background: "var(--accent-soft)" } : undefined}
                  data-testid="document-passage"
                >
                  <span className="flex items-center gap-2 min-w-0">
                    <span className="label font-mono shrink-0">
                      {c.part + 1} · {c.char_start.toLocaleString()}–{c.char_end.toLocaleString()}
                    </span>
                    {!c.has_embedding && <span className="label">not in meaning search</span>}
                    <span className="flex-1" />
                    <CopyButton value={c.chunk_id} label="Copy passage id" />
                  </span>
                  <p className="text-[12.5px] whitespace-pre-wrap break-words" style={{ color: "var(--ink-dim)" }}>
                    {text}
                    {c.char_end - c.char_start > 600 ? "…" : ""}
                  </p>
                </li>
              );
            })}
          </ol>
        )}
        {detail.text_truncated && <p className="label">The text is long: the list shows the passages, Download has the whole file.</p>}
      </div>
    </section>
  );
}

// ---- page ----------------------------------------------------------------------------------

// useSearchParams needs a Suspense boundary for the static build.
export default function DocumentsPage() {
  return (
    <Suspense>
      <Documents />
    </Suspense>
  );
}

function Documents() {
  const router = useRouter();
  const params = useSearchParams();
  const selected = params.get("id");
  const chunk = params.get("chunk");
  const listQuery = useDocuments();
  const list = listQuery.data;
  const docs = list?.results ?? (listQuery.isError ? [] : null);
  const maxBytes = list?.max_bytes ?? null;
  const storageOff = list !== undefined && !list.storage;
  const select = (id: string | null) => router.replace(id ? `/documents?id=${encodeURIComponent(id)}` : "/documents", { scroll: false });
  const uploads = useUploads(maxBytes, (id) => select(id));
  const input = useRef<HTMLInputElement>(null);

  return (
    <div className="max-w-6xl flex flex-col gap-8">
      <PageHeader
        icon={FilesIcon} tone="company"
        title="Documents"
        intro="Upload notes, specs and exports. Your AI apps can find each passage and trace it back to the file. Only you can read your documents."
        actions={
          <button type="button" onClick={() => input.current?.click()} disabled={storageOff} className="btn btn-primary">
            <UploadSimpleIcon {...ICON} aria-hidden />
            Upload
          </button>
        }
      />
      <>
        <input
          ref={input}
          type="file"
          multiple
          accept={ACCEPT}
          className="sr-only"
          tabIndex={-1}
          aria-hidden
          onChange={(e) => {
            if (e.target.files?.length) uploads.add(e.target.files);
            e.target.value = "";
          }}
        />
      </>

      {listQuery.isError && <ErrorLine error={failure(listQuery.error, "Could not load your documents.", " Reload the page to try again.")} />}
      {storageOff && (
        <div className="ledger px-4 py-3 text-[13px]" style={{ color: "var(--ink-dim)" }}>
          Document storage is turned off on this server, so files can&apos;t be uploaded yet. Ask the person who installed Eunomia to turn it on with <code className="font-mono text-[12px]">EUNOMIA_DOCUMENTS_BACKEND</code>{" "}
          (see Documents in Help).
        </div>
      )}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_380px] items-start">
        <div className="flex flex-col gap-6 min-w-0">
          {!storageOff && <DropZone maxBytes={maxBytes} disabled={list === undefined} onFiles={uploads.add} />}
          <UploadRows rows={uploads.rows} onDismiss={uploads.dismiss} />

          <section className="flex flex-col gap-2.5" aria-labelledby="documents-title">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <h2 id="documents-title" className="section-title">
                Your documents
              </h2>
              {docs && (
                <span className="label">
                  {(list?.total ?? docs.length).toLocaleString()} {(list?.total ?? docs.length) === 1 ? "document" : "documents"} · Only you can read these
                </span>
              )}
            </div>
            {docs?.length === 0 ? (
              <EmptyState
                variant="documents"
                title="No documents yet"
                action={
                  !storageOff && (
                    <button type="button" onClick={() => input.current?.click()} className="btn btn-sm">
                      Choose a file
                    </button>
                  )
                }
              >
                Drop a file above, or ask your AI app to save one for you. Each passage becomes searchable and links back to the file.
              </EmptyState>
            ) : (
            <div className="ledger overflow-x-auto">
              <table className="data-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th className="hidden sm:table-cell">Type</th>
                    <th className="hidden md:table-cell text-right">Size</th>
                    <th className="hidden md:table-cell">Uploaded</th>
                    <th>Status</th>
                    <th className="hidden sm:table-cell text-right">Sections</th>
                  </tr>
                </thead>
                <tbody aria-busy={docs === null}>
                  {docs === null &&
                    [0, 1].map((i) => (
                      <tr key={i} aria-hidden>
                        <td><span className="skeleton block h-4" style={{ width: "60%" }} /></td>
                        <td className="hidden sm:table-cell"><span className="skeleton block h-4 w-16" /></td>
                        <td className="hidden md:table-cell"><span className="skeleton block h-4 w-12 ml-auto" /></td>
                        <td className="hidden md:table-cell"><span className="skeleton block h-4 w-16" /></td>
                        <td><span className="skeleton block h-4 w-14" /></td>
                        <td className="hidden sm:table-cell"><span className="skeleton block h-4 w-8 ml-auto" /></td>
                      </tr>
                    ))}
                  {docs?.map((d) => {
                    const on = d.id === selected;
                    return (
                      <tr key={d.id} style={on ? { background: "var(--accent-soft)" } : undefined} data-testid="document-row">
                        <td className="max-w-0 w-full">
                          <button
                            type="button"
                            onClick={() => select(on ? null : d.id)}
                            aria-current={on || undefined}
                            className="flex items-center gap-2 min-h-11 min-w-0 max-w-full text-left font-medium hover:underline underline-offset-2"
                            style={{ color: "var(--accent-text)" }}
                          >
                            <FileTextIcon {...ICON} aria-hidden className="shrink-0" />
                            <span className="truncate">{d.filename}</span>
                          </button>
                        </td>
                        <td className="hidden sm:table-cell whitespace-nowrap" style={{ color: "var(--ink-dim)" }}>
                          {TYPE_LABEL[d.media_type] ?? d.media_type}
                        </td>
                        <td className="hidden md:table-cell text-right font-mono text-[12px] whitespace-nowrap" style={{ color: "var(--ink-dim)" }}>
                          {size(d.size_bytes)}
                        </td>
                        <td className="hidden md:table-cell font-mono text-[12px] whitespace-nowrap" style={{ color: "var(--ink-dim)" }}>
                          {ago(d.created_at)}
                        </td>
                        <td>
                          <Status status={d.status} />
                        </td>
                        <td className="hidden sm:table-cell text-right font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                          {d.status === "ready" ? d.chunk_count.toLocaleString() : "–"}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            )}
          </section>
        </div>

        <aside className="lg:sticky lg:top-8 min-w-0">
          {selected ? (
            <DocumentPanel key={selected} id={selected} chunk={chunk} onClose={() => select(null)} />
          ) : (
            <p className="soft-panel text-[14px] px-4 py-3" style={{ color: "var(--ink-dim)" }}>
              Choose a document to read its passages, download it, export it or delete it.
            </p>
          )}
        </aside>
      </div>
    </div>
  );
}
