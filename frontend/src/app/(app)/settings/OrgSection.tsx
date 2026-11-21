"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { CodeField } from "@/components/mfa/Mfa";
import { useOrg, useRenameOrg, useSetOrgSecurity } from "@/lib/queries/org";
import { PanelHead, StatusLine } from "./shared";

/** The organisation's own settings, for its owner and admins: its name (the owner renames it) and whether its
 * members must use two-step verification. Both are this organisation's alone. */
export function OrgSection() {
  return (
    <div className="flex flex-col gap-8" data-testid="org-settings">
      <OrgName />
      <OrgSecurity />
    </div>
  );
}

function OrgName() {
  const org = useOrg();
  const rename = useRenameOrg();
  const o = org.data;
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const [saved, setSaved] = useState(false);
  const value = draft ?? o?.name ?? "";

  async function save(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSaved(false);
    try {
      await rename.mutateAsync(value);
      setDraft(null);
      setSaved(true);
    } catch (err) {
      setError(failure(err, "Could not rename the organisation."));
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <PanelHead title="Organisation">
        The name people see when you invite them. Only the organisation&apos;s owner can change it. A workspace made at sign-up is named after its founder&apos;s email until it is renamed.
      </PanelHead>
      {org.isError && <ErrorLine error={failure(org.error, "Could not load the organisation.")} />}
      {o && (
        <form onSubmit={save} className="flex flex-col gap-3 max-w-[28rem]">
          <label htmlFor="org-name" className="label">
            Name
          </label>
          <input
            id="org-name"
            className="field h-10 px-3 text-[14px]"
            value={value}
            maxLength={80}
            readOnly={!o.can_rename}
            onChange={(e) => {
              setDraft(e.target.value);
              setSaved(false);
            }}
          />
          {!o.can_rename && (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              You are an {o.your_role} of this organisation. Ask its owner to rename it.
            </p>
          )}
          {error && <ErrorLine error={error} />}
          {o.can_rename && (
            <div className="flex items-center gap-3">
              <button type="submit" className="btn btn-primary" disabled={rename.isPending || !value.trim() || value.trim() === o.name}>
                {rename.isPending ? "Saving…" : "Save name"}
              </button>
              {saved && (
                <span role="status" className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                  Saved.
                </span>
              )}
            </div>
          )}
        </form>
      )}
    </div>
  );
}

/** Require two-step verification of this organisation's members. Binds only them, on top of the server-wide switch. */
function OrgSecurity() {
  const org = useOrg();
  const set = useSetOrgSecurity();
  const o = org.data;
  const [stepUp, setStepUp] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<Failure | null>(null);

  async function change(on: boolean) {
    setError(null);
    try {
      await set.mutateAsync({ require_mfa: on, ...(stepUp && code.trim() ? { mfa_code: code.trim() } : {}) });
      setStepUp(false);
      setCode("");
    } catch (err) {
      const f = failure(err, "Could not change the setting. Try again.");
      if (f.code === "auth.step_up_required") setStepUp(true);
      else setError(f);
      setCode("");
    }
  }

  return (
    <div className="flex flex-col gap-4" data-testid="org-require-mfa">
      <PanelHead
        title="Two-step verification for this organisation"
        status={o && <StatusLine tone={o.require_mfa ? "good" : "off"}>{o.require_mfa ? "Required" : "Optional"}</StatusLine>}
      >
        Every member of this organisation who signs in with a password must use two-step verification. It binds this organisation only.
        People who sign in with single sign-on are left to your identity provider. API tokens and connected apps keep working. Set up
        your own authenticator first (Settings &gt; Two-step verification).
      </PanelHead>
      {o && (
        <>
          {o.server_requires_mfa && (
            <p className="text-body" style={{ color: "var(--ink-dim)" }}>
              The server&apos;s operator already requires two-step verification of everyone on this server, so turning this off here would not
              excuse anyone.
            </p>
          )}
          {o.without_mfa > 0 && (
            <p className="text-body" style={{ color: "var(--ink-dim)" }}>
              {o.without_mfa} {o.without_mfa === 1 ? "member hasn't" : "members haven't"} set it up yet.
              {o.require_mfa ? " They'll be asked at their next sign-in." : " Turning this on signs them out until they do."}
            </p>
          )}
          {stepUp && (
            <>
              <p role="status" className="text-body" style={{ color: "var(--ink-dim)" }}>
                Confirm it&apos;s you: enter a code from your authenticator app (or a recovery code).
              </p>
              <CodeField id="org-require-mfa-code" label="Code" recovery value={code} onChange={setCode} />
            </>
          )}
          {error && <ErrorLine error={error} />}
          <button
            type="button"
            disabled={set.isPending || (stepUp && !code.trim())}
            className={o.require_mfa ? "btn btn-ghost self-start" : "btn btn-primary self-start"}
            onClick={() => change(!o.require_mfa)}
          >
            {set.isPending ? "Saving…" : o.require_mfa ? "Stop requiring two-step verification" : "Require two-step verification"}
          </button>
        </>
      )}
    </div>
  );
}
