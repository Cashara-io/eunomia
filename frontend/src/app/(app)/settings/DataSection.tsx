"use client";

import { DownloadSimpleIcon, XIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { downloadExport, downloadSubjectExport } from "@/lib/api";
import type { ExportRow } from "@/lib/gen";
import { useMe } from "@/lib/queries/auth";
import { useEraseMe, useMyDataActivity, useStartExport, useSubjectExports } from "@/lib/queries/subject";
import { ICON, PanelHead, StatusLine } from "./shared";

const when = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/* "12 memories, 3 documents, …": the manifest's non-empty categories in words. */
function summary(e: ExportRow): string {
  const cats = (e.manifest as { categories?: { name: string; count: number }[] })?.categories ?? [];
  const parts = cats.filter((c) => c.count > 0).map((c) => `${c.count} ${c.name.replace(/_/g, " ")}`);
  return parts.length ? parts.join(", ") : "Nothing yet";
}

/* One export: when, what is in it, and Download once it is ready. */
function ExportLine({ e }: { e: ExportRow }) {
  const [error, setError] = useState<Failure | null>(null);
  // an export you started as an admin holds this workspace's data about that person, not their own data
  const forSomeoneElse = e.scope === "organisation";
  return (
    <li className="py-3 flex flex-col gap-1 min-w-0" data-testid="subject-export">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[13px] tabular-nums" style={{ color: "var(--ink-dim)" }}>
          {when(e.created_at)}
          {forSomeoneElse ? " · this workspace's data about someone" : ""} · kept until {when(e.expires_at)}
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
  // the server said this account is the only member of a workspace: deleting it closes that workspace
  const [closes, setCloses] = useState(false);
  const [closeOk, setCloseOk] = useState(false);
  const hasPassword = me.data?.has_password ?? true;
  // single sign-on only: the proof is a sign-in from the last 10 minutes
  const [reauth, setReauth] = useState(false);

  async function confirm() {
    setError(null);
    try {
      await erase.mutateAsync({ ...(hasPassword ? { password: secret } : {}), close_workspace: closes && closeOk });
      // signed out everywhere: nothing cached for this account is valid any more
      qc.clear();
      router.replace("/login?erased=1");
    } catch (e) {
      if ((e as { code?: string }).code === "subject.workspace_closes") {
        setCloses(true);
        return;
      }
      if ((e as { code?: string }).code === "subject.reauthenticate") {
        setReauth(true);
        return;
      }
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
          {hasPassword ? (
            <label className="flex flex-col gap-1 max-w-[28rem]">
              <span className="label">Your password</span>
              <input type="password" autoComplete="current-password" className="field h-10 px-3" value={secret} onChange={(e) => setSecret(e.target.value)} aria-label="Your password" />
            </label>
          ) : (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              You sign in with single sign-on, so the confirmation is a sign-in from the last 10 minutes.
            </p>
          )}
          {reauth && (
            <p className="text-[13px]" role="status" data-testid="reauthenticate">
              Your sign-in is older than 10 minutes.{" "}
              <a className="underline" href="/login?next=%2Fsettings%3Ftab%3Ddata">
                Sign in again
              </a>
              , then come back here to confirm.
            </p>
          )}
          {closes && (
            <label className="flex items-start gap-2 text-[13px] max-w-[60ch]" data-testid="close-workspace">
              <input type="checkbox" className="mt-1" checked={closeOk} onChange={(e) => setCloseOk(e.target.checked)} />
              <span>
                You&apos;re the only person in your workspace, so it closes with your account: every space, document and connected
                source in it is deleted, and nobody can use it again. I understand.
              </span>
            </label>
          )}
          {error && <ErrorLine error={error} />}
          <div className="flex items-center gap-2">
            <button type="submit" className="btn btn-danger-solid" disabled={(hasPassword && !secret) || erase.isPending || (closes && !closeOk)}>
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
              <ExportLine key={e.id} e={e} />
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
      <DataActivity />
      <DeleteAccount />
    </div>
  );
}

const ACTIVITY: Record<string, string> = {
  "subject.export_request": "You exported your data",
  "subject.export_request_admin": "An admin exported this workspace's data about you",
  "subject.erase_request": "Deleting your data was requested",
  "subject.legal_hold": "A legal hold was placed on your account",
  "subject.legal_hold_lift": "The legal hold on your account was lifted",
  "user.suspend": "Your account was suspended",
  "user.restore": "Your account was restored",
  "admin.grant": "You were made an admin",
  "admin.revoke": "Your admin rights were taken back",
  "admin.token_revoke": "An admin revoked one of your API tokens",
};

/* What has been done about your data, newest first: what an admin exported, holds, deletion requests. */
function DataActivity() {
  const q = useMyDataActivity();
  const rows = (q.data ?? []).filter((r) => ACTIVITY[r.action]);
  if (!rows.length) return null;
  return (
    <div className="flex flex-col gap-2" data-testid="data-activity">
      <h3 className="text-reading font-semibold">What has been done with your data</h3>
      <ul className="hairline-rows text-[13px]">
        {rows.map((r, i) => (
          <li key={i} className="py-2 flex flex-wrap gap-x-3">
            <span className="tabular-nums" style={{ color: "var(--ink-dim)" }}>
              {r.created_at ? when(r.created_at) : ""}
            </span>
            <span>{ACTIVITY[r.action]}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
