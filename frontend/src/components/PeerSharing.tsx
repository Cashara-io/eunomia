"use client";

// Sharing a memory space with another Eunomia (docs/peer-vaults.md), in plain words: a one-time code
// to send, the other Eunomias this space is shared with and how syncing is going, and a dialog to
// join a space someone else shared.
import { useEffect, useId, useRef, useState } from "react";
import { LogIn, RefreshCw, Share2, Unlink, X } from "lucide-react";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import StatusLabel, { type StatusTone } from "@/components/ui/StatusLabel";
import CopyButton from "@/components/bits/CopyButton";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import {
  useCreateShareCode,
  useDeleteShareCode,
  useJoinShared,
  useStopPeer,
  useSyncPeer,
  useVaultPeers,
  type PeerLink,
  type PeerRole,
  type ShareCode,
} from "@/lib/queries/peers";

const ICON = { size: 16, strokeWidth: 1.75, "aria-hidden": true } as const;

const ROLE_WORDS: Record<PeerRole, string> = { edit: "Can add and change", read: "Can only read" };

function ago(iso: string | null): string | null {
  if (!iso) return null;
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 45) return "just now";
  if (s < 90) return "a minute ago";
  if (s < 3600) return `${Math.round(s / 60)} minutes ago`;
  if (s < 5400) return "an hour ago";
  if (s < 86400) return `${Math.round(s / 3600)} hours ago`;
  return new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

function day(iso: string | null): string {
  return iso ? new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "long" }) : "";
}

/** The status of one link, always in words. */
function statusWords(link: PeerLink): [StatusTone, string] {
  switch (link.status) {
    case "up_to_date":
      return ["good", "Up to date"];
    case "syncing":
      return ["info", "Syncing…"];
    case "unreachable":
      return link.direction === "guest"
        ? ["warning", "Can't reach the other Eunomia, will keep trying"]
        : ["warning", "Haven't heard from the other Eunomia lately"];
    case "integrity_problem":
      return ["critical", "A change failed its integrity check"];
    case "revoked":
      return ["neutral", "Sharing stopped. Your copy stays here."];
  }
}

function matchWords(link: PeerLink): string {
  if (link.status === "revoked") return "";
  if (link.digests_match === true) return "Copies match ✓";
  if (link.digests_match === false) return "Copies differ — syncing";
  return "Checking…";
}

function PeerRow({ link, isOwner, vaultId, vaultName }: { link: PeerLink; isOwner: boolean; vaultId: string; vaultName: string }) {
  const sync = useSyncPeer(vaultId);
  const stop = useStopPeer(vaultId);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [tone, words] = statusWords(link);
  const synced = ago(link.last_sync_at);
  const name = link.peer_name || "The other Eunomia";
  const guest = link.direction === "guest";

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <span className="text-[15px] font-medium [overflow-wrap:anywhere]">{name}</span>
        <span className="text-[12px] font-mono" style={{ color: "var(--ink-dim)" }} title="This Eunomia's ID, to compare with what it shows">
          ID {link.peer_fingerprint}
        </span>
      </div>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        {guest ? `This space is a copy from ${name}.` : `${name} has a copy of this space.`} {ROLE_WORDS[link.role]}.
        {guest && link.role === "read" && " Changes made here stay on this Eunomia."}
      </p>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <StatusLabel tone={tone}>{words}</StatusLabel>
        {matchWords(link) && <span className="text-[13px]">{matchWords(link)}</span>}
      </div>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        {synced ? `Last synced ${synced}` : "Not synced yet"}
        {link.address && guest ? ` · via ${link.address}` : ""}
        {link.last_report?.documents_not_shared ? " · Documents aren't shared yet" : ""}
      </p>
      {link.status === "unreachable" && link.last_error && guest && (
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          Last problem: {link.last_error}
        </p>
      )}
      {error && <ErrorLine error={error} />}
      <div className="flex flex-wrap gap-2">
        {guest && link.status !== "revoked" && (
          <button
            type="button"
            className="btn btn-sm"
            disabled={sync.isPending}
            aria-busy={sync.isPending}
            onClick={() => {
              setError(null);
              sync.mutate(link.id, { onError: (e) => setError(failure(e, "Couldn't sync just now.", " It will try again by itself.")) });
            }}
          >
            <RefreshCw {...ICON} />
            {sync.isPending ? "Syncing…" : "Sync now"}
          </button>
        )}
        {isOwner && (
          <button type="button" className="btn btn-sm" onClick={() => setConfirm(true)}>
            <Unlink {...ICON} />
            {link.status === "revoked" ? "Remove" : "Stop sharing…"}
          </button>
        )}
      </div>
      <ConfirmDialog
        open={confirm}
        title={guest ? `Stop syncing “${vaultName}” with ${name}?` : `Stop sharing “${vaultName}” with ${name}?`}
        confirmLabel="Stop sharing"
        busy={stop.isPending}
        error={stop.error ? failure(stop.error, "Couldn't stop sharing.").message : undefined}
        onCancel={() => setConfirm(false)}
        onConfirm={() => stop.mutate(link.id, { onSuccess: () => setConfirm(false) })}
      >
        <p>
          {guest
            ? `Changes will no longer travel between this space and ${name}. Your copy stays here as a normal space, and theirs stays with them.`
            : `${name} will stop getting changes from this space, and its changes will no longer come here. It keeps the copy it already has.`}
        </p>
      </ConfirmDialog>
    </li>
  );
}

/** "Share with another Eunomia", for the selected space's inspector. */
export default function PeerSharing({ vault }: { vault: { id: string; name: string; role: string } }) {
  const isOwner = vault.role === "owner";
  const peers = useVaultPeers(vault.id);
  const create = useCreateShareCode(vault.id);
  const withdraw = useDeleteShareCode(vault.id);
  const [role, setRole] = useState<PeerRole>("edit");
  const [made, setMade] = useState<ShareCode | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const roleId = useId();
  const codeId = useId();
  const links = peers.data?.links ?? [];
  const codes = (peers.data?.codes ?? []).filter((c) => c.id !== made?.id);

  return (
    <section className="flex flex-col gap-2 pt-4" aria-labelledby={`${roleId}-title`}>
      <h3 id={`${roleId}-title`} className="section-title text-[15px] flex items-center gap-1.5">
        <Share2 {...ICON} />
        Share with another Eunomia
      </h3>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        Keep a full copy of this space on another Eunomia, and keep both copies in step. Documents aren&apos;t shared yet.
      </p>

      {links.length > 0 && (
        <ul className="hairline-rows" aria-label="Eunomias this space is shared with">
          {links.map((l) => (
            <PeerRow key={l.id} link={l} isOwner={isOwner} vaultId={vault.id} vaultName={vault.name} />
          ))}
        </ul>
      )}
      {peers.isError && <ErrorLine error={failure(peers.error, "Couldn't load who this space is shared with.")} />}

      {isOwner && (
        <div className="flex flex-col gap-2 pt-1">
          <label htmlFor={roleId} className="text-[13px] font-medium">
            The other Eunomia
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <select id={roleId} className="field h-11 px-3 text-[14px]" value={role} onChange={(e) => setRole(e.target.value as PeerRole)}>
              <option value="edit">{ROLE_WORDS.edit}</option>
              <option value="read">{ROLE_WORDS.read}</option>
            </select>
            <button
              type="button"
              className="btn btn-primary"
              disabled={create.isPending}
              aria-busy={create.isPending}
              onClick={() => {
                setError(null);
                create.mutate(role, {
                  onSuccess: (c) => setMade(c),
                  onError: (e) => setError(failure(e, "Couldn't create a share code.")),
                });
              }}
            >
              {create.isPending ? "Creating…" : "Create share code"}
            </button>
          </div>
          {error && <ErrorLine error={error} />}
          {made && (
            <div className="flex flex-col gap-1.5">
              <label htmlFor={codeId} className="text-[13px] font-medium">
                Share code ({ROLE_WORDS[made.role].toLowerCase()})
              </label>
              <div className="flex items-start gap-2">
                <textarea
                  id={codeId}
                  readOnly
                  rows={3}
                  value={made.code}
                  onFocus={(e) => e.currentTarget.select()}
                  className="field flex-1 min-w-0 px-3 py-2 text-[12px] font-mono break-all resize-none"
                />
                <CopyButton value={made.code} label="Copy" size="sm" />
              </div>
              <p className="text-[13px]">
                Send this code to the person running the other Eunomia. It works once and expires on {day(made.expires_at)}.
              </p>
              <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                This Eunomia&apos;s ID is <span className="font-mono">{made.fingerprint}</span>. The other side will show it after joining.
              </p>
            </div>
          )}
          {codes.length > 0 && (
            <div className="flex flex-col gap-1 pt-1">
              <span className="text-[13px] font-medium">Codes not used yet</span>
              <ul className="flex flex-col gap-1">
                {codes.map((c) => (
                  <li key={c.id} className="flex flex-wrap items-center justify-between gap-2 text-[13px]">
                    <span>
                      {ROLE_WORDS[c.role]} · expires {day(c.expires_at)}
                    </span>
                    <button type="button" className="btn btn-sm" disabled={withdraw.isPending} onClick={() => withdraw.mutate(c.id)}>
                      <X {...ICON} />
                      Cancel code
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

/** "Join a shared space": a button for the page header and a dialog to paste a share code into. */
export function JoinSharedSpace({ onJoined }: { onJoined?: (vaultId: string) => void }) {
  const join = useJoinShared();
  const [open, setOpen] = useState(false);
  const [code, setCode] = useState("");
  const [joined, setJoined] = useState<string | null>(null);
  const ref = useRef<HTMLDialogElement>(null);
  const opener = useRef<Element | null>(null);
  const titleId = useId();
  const codeId = useId();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      opener.current = document.activeElement;
      d.showModal();
    } else if (!open && d.open) {
      d.close();
      (opener.current as HTMLElement | null)?.focus?.();
    }
  }, [open]);

  const close = () => {
    if (join.isPending) return;
    setOpen(false);
    setCode("");
    setJoined(null);
    join.reset();
  };

  return (
    <>
      <button type="button" className="btn" onClick={() => setOpen(true)}>
        <LogIn {...ICON} />
        Join a shared space
      </button>
      <dialog
        ref={ref}
        className="bauhaus-dialog m-auto"
        aria-labelledby={titleId}
        onCancel={(e) => {
          e.preventDefault();
          close();
        }}
      >
        <form
          method="dialog"
          className="flex flex-col gap-4 p-6"
          onSubmit={(e) => {
            e.preventDefault();
            if (joined) return close();
            if (!code.trim() || join.isPending) return;
            join.mutate(code.trim(), {
              onSuccess: (out) => {
                setJoined(out.vault.name);
                onJoined?.(out.vault.id);
              },
            });
          }}
        >
          <h2 id={titleId} className="text-[20px] font-semibold leading-tight tracking-[-0.01em]">
            Join a shared space
          </h2>
          {joined ? (
            <p className="text-[14px]" role="status">
              Joined. “{joined}” is now in your spaces, and it will stay in step with the other Eunomia by itself.
            </p>
          ) : (
            <>
              <p className="text-[14px] leading-relaxed">
                Paste the share code you were sent. This Eunomia makes a new space with a full copy and keeps it in step.
              </p>
              <div className="flex flex-col gap-1.5">
                <label htmlFor={codeId} className="text-[14px] font-medium">
                  Share code
                </label>
                <textarea
                  id={codeId}
                  rows={4}
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  placeholder="eunomia1:…"
                  autoComplete="off"
                  spellCheck={false}
                  className="field px-3 py-2 text-[13px] font-mono break-all"
                />
              </div>
              {join.isPending && (
                <p className="text-[13px]" role="status">
                  Connecting to the other Eunomia…
                </p>
              )}
              {join.isError && <ErrorLine error={failure(join.error, "Couldn't join.")} />}
            </>
          )}
          <div className="flex flex-wrap justify-end gap-2 pt-1">
            {!joined && (
              <button type="button" className="btn" onClick={close} disabled={join.isPending}>
                Cancel
              </button>
            )}
            <button type="submit" className="btn btn-primary" disabled={(!joined && !code.trim()) || join.isPending} aria-busy={join.isPending}>
              {joined ? "Done" : join.isPending ? "Joining…" : "Join"}
            </button>
          </div>
        </form>
      </dialog>
    </>
  );
}
