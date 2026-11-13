"use client";

import { DownloadSimpleIcon, XIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { downloadExport, downloadSubjectExport } from "@/lib/api";
import type { ExportRow } from "@/lib/gen";
import { useMe } from "@/lib/queries/auth";
import { useEraseMe, useStartExport, useSubjectExports } from "@/lib/queries/subject";
import { ICON, PanelHead, StatusLine } from "./shared";

const when = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/* "12 memories, 3 documents, …": the manifest's non-empty categories in words. */
function summary(e: ExportRow): string {
  const cats = (e.manifest as { categories?: { name: string; count: number }[] })?.categories ?? [];
  const parts = cats.filter((c) => c.count > 0).map((c) => `${c.count} ${c.name.replace(/_/g, " ")}`);
  return parts.length ? parts.join(", ") : "Nothing yet";
}

/* One export: when, what is in it, and Download once it is ready. */
function ExportLine({ e, you }: { e: ExportRow; you: string }) {
  const [error, setError] = useState<Failure | null>(null);
  const forSomeoneElse = e.subject && e.subject !== you;
  return (
    <li className="py-3 flex flex-col gap-1 min-w-0" data-testid="subject-export">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[13px] tabular-nums" style={{ color: "var(--ink-dim)" }}>
          {when(e.created_at)}
          {forSomeoneElse ? " · for someone in your workspace" : ""} · kept until {when(e.expires_at)}
        </span>
        {e.status === "ready" ? (
          <button
            type="button"
            className="btn btn-sm"
            onClick={async () => {
              setError(null);
              try {
                await downloadSubjectExport(e.id);
              } catch (err) {
                setError(failure(err, "Could not download the export.", " Try again in a moment."));
              }
            }}
          >
            <DownloadSimpleIcon {...ICON} />
            Download
          </button>
        ) : (
          <StatusLine tone={e.status === "failed" ? "critical" : "warning"}>{e.status === "failed" ? "Failed" : "Preparing…"}</StatusLine>
        )}
      </div>
      {e.status === "ready" && <span className="text-[13px]">{summary(e)}</span>}
      {e.status === "failed" && e.error && (
        <span className="text-[13px]" style={{ color: "var(--critical)" }}>
          {e.error}
        </span>
      )}
      {error && <ErrorLine error={error} />}
    </li>
  );
}

/* Delete my account: a quiet button, then what happens, the password, and the final confirm. */
function DeleteAccount() {
  const me = useMe();
  const erase = useEraseMe();
  const router = useRouter();
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [secret, setSecret] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const hasPassword = me.data?.has_password ?? true;

  async function confirm() {
    setError(null);
    try {
      await erase.mutateAsync(hasPassword ? { password: secret } : { confirm_email: secret });
      // signed out everywhere: nothing cached for this account is valid any more
      qc.clear();
      router.replace("/login?erased=1");
    } catch (e) {
      setError(failure(e, "Your account was not deleted."));
    }
  }

  return (
    <div className="ledger px-5 py-4 flex flex-col gap-3" data-testid="delete-account">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div className="text-reading font-semibold">Delete my account and data</div>
          <div className="text-[13px] mt-0.5" style={{ color: "var(--ink-dim)" }}>
            Removes your account and everything that is yours alone. This can&apos;t be undone.
          </div>
        </div>
        {!open && (
          <button type="button" className="btn btn-danger" onClick={() => setOpen(true)}>
            Delete my account
          </button>
        )}
      </div>
      {open && (
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void confirm();
          }}
        >
          <ul className="text-[13px] list-disc pl-5 flex flex-col gap-1">
            <li>You are signed out everywhere at once. Your API tokens and connected apps stop working.</li>
            <li>Your personal space, your chats, sources, recordings, settings and uploaded files are deleted.</li>
            <li>
              What you wrote in spaces you share stays for the others, with you removed as its author. Your email address is
              removed from what stays.
            </li>
            <li>A record of the deletion is kept, with counts only and no way to tell it was you.</li>
            <li>Download an export first if you want a copy. Backups taken before today still hold your data until they rotate out.</li>
          </ul>
          <label className="flex flex-col gap-1 max-w-[28rem]">
            <span className="label">{hasPassword ? "Your password" : "Type your email to confirm"}</span>
            <input
              type={hasPassword ? "password" : "email"}
              autoComplete={hasPassword ? "current-password" : "off"}
              className="field h-10 px-3"
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              aria-label={hasPassword ? "Your password" : "Your email"}
            />
          </label>
          {error && <ErrorLine error={error} />}
          <div className="flex items-center gap-2">
            <button type="submit" className="btn btn-danger-solid" disabled={!secret || erase.isPending}>
              {erase.isPending ? "Deleting…" : "Delete my account and data"}
            </button>
            <button
              type="button"
              className="btn btn-ghost btn-icon"
              aria-label="Cancel"
              onClick={() => {
                setOpen(false);
                setSecret("");
                setError(null);
              }}
            >
              <XIcon {...ICON} />
            </button>
          </div>
        </form>
      )}
    </div>
  );
}

export function DataSection() {
  const me = useMe();
  const exports = useSubjectExports();
  const start = useStartExport();
  const [error, setError] = useState<Failure | null>(null);
  const [quick, setQuick] = useState(false);
  const pending = exports.data?.some((e) => e.status === "pending") ?? false;

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="Your data">
        Get a copy of everything Eunomia holds about you, or delete your account and the data that is yours alone.
      </PanelHead>
      <div className="ledger px-5 py-4 flex flex-col gap-2">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="text-reading font-semibold">Export my data</div>
            <div className="text-[13px] mt-0.5 max-w-[60ch]" style={{ color: "var(--ink-dim)" }}>
              One zip file: your account, spaces, what you wrote in spaces you can still read, documents and their files, synced
              records, chats, sources, tokens and connected apps by name, and your activity. A manifest lists what is in it and
              what was left out.
            </div>
          </div>
          <button
            type="button"
            className="btn"
            disabled={start.isPending || pending}
            onClick={async () => {
              setError(null);
              try {
                await start.mutateAsync();
              } catch (e) {
                setError(failure(e, "Could not start the export.", " Try again in a moment."));
              }
            }}
          >
            <DownloadSimpleIcon {...ICON} />
            {pending ? "Preparing…" : "Export my data"}
          </button>
        </div>
        {error && <ErrorLine error={error} />}
        {exports.isError && <ErrorLine error={failure(exports.error, "Could not load your exports. Reload to try again.")} />}
        {exports.data && exports.data.length > 0 && (
          <ul className="hairline-rows" aria-label="Your exports">
            {exports.data.map((e) => (
              <ExportLine key={e.id} e={e} you={me.data?.id ?? ""} />
            ))}
          </ul>
        )}
        <button
          type="button"
          className="self-start text-[13px] underline"
          style={{ color: "var(--ink-dim)" }}
          onClick={async () => {
            setQuick(true);
            setError(null);
            try {
              await downloadExport();
            } catch (e) {
              setError(failure(e, "Could not prepare the export.", " Try again in a moment."));
            } finally {
              setQuick(false);
            }
          }}
        >
          {quick ? "Preparing…" : "Or download just your personal space as JSON"}
        </button>
      </div>
      <DeleteAccount />
    </div>
  );
}
