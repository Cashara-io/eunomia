"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { CodeField } from "@/components/mfa/Mfa";
import { useDeleteOrgAiProvider, useOrg, useOrgAi, useRenameOrg, useSetOrgAiEnabled, useSetOrgAiProvider, useSetOrgSecurity } from "@/lib/queries/org";
import { PanelHead, StatusLine } from "./shared";

/** The organisation's own settings, for its owner and admins: its name (the owner renames it) and whether its
 * members must use two-step verification. Both are this organisation's alone. */
export function OrgSection() {
  return (
    <div className="flex flex-col gap-8" data-testid="org-settings">
      <OrgName />
      <OrgSecurity />
      <OrgAi />
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

const USAGE_WORDS: Record<string, { tone: "good" | "warning" | "critical"; text: string }> = {
  ok: { tone: "good", text: "Within this month's allowance" },
  nearly_used: { tone: "warning", text: "Nearly used this month's allowance" },
  used_up: { tone: "critical", text: "This month's allowance is used up" },
};

/** Model use for this organisation: this month's allowance on the server's key (a state and a share, never an
 * amount), the organisation's own provider key, and a switch that turns AI off for everyone. */
function OrgAi() {
  const ai = useOrgAi();
  const setProvider = useSetOrgAiProvider();
  const removeProvider = useDeleteOrgAiProvider();
  const setEnabled = useSetOrgAiEnabled();
  const a = ai.data;
  const [form, setForm] = useState<{ base_url: string; api_key: string; chat_model: string; embedding_model: string } | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const [saved, setSaved] = useState(false);
  const draft = form ?? { base_url: a?.provider.base_url ?? "", api_key: "", chat_model: a?.provider.chat_model ?? "", embedding_model: a?.provider.embedding_model ?? "" };
  const usage = a?.usage.state ? USAGE_WORDS[a.usage.state] : null;

  async function save(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSaved(false);
    try {
      await setProvider.mutateAsync({
        base_url: draft.base_url,
        chat_model: draft.chat_model,
        embedding_model: draft.embedding_model,
        // left empty, the stored key is kept: it is never shown, so it cannot be edited in place
        ...(draft.api_key.trim() ? { api_key: draft.api_key.trim() } : {}),
      });
      setForm(null);
      setSaved(true);
    } catch (err) {
      setError(failure(err, "Could not save the provider."));
    }
  }

  async function run(fn: () => Promise<unknown>, fallback: string) {
    setError(null);
    setSaved(false);
    try {
      await fn();
      setForm(null);
    } catch (err) {
      setError(failure(err, fallback));
    }
  }

  const field = (id: string, label: string, key: keyof typeof draft, props: React.InputHTMLAttributes<HTMLInputElement> = {}) => (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="label">
        {label}
      </label>
      <input id={id} className="field h-10 px-3 text-[14px]" value={draft[key]} onChange={(e) => setForm({ ...draft, [key]: e.target.value })} {...props} />
    </div>
  );

  return (
    <div className="flex flex-col gap-4" data-testid="org-ai">
      <PanelHead
        title="AI for this organisation"
        status={a && <StatusLine tone={!a.enabled ? "off" : (usage?.tone ?? "good")}>{!a.enabled ? "Off" : (usage?.text ?? "On")}</StatusLine>}
      >
        Saving what people write, extracting facts, search by meaning and the in-app chat call a language model. By default they use this
        server&apos;s provider key, up to a monthly allowance. When the allowance is used up those features pause until it renews at the start of
        next month (UTC), or until you add your organisation&apos;s own key below. Saving, reading and keyword search never need a model.
      </PanelHead>
      {ai.isError && <ErrorLine error={failure(ai.error, "Could not load the AI settings.")} />}
      {a && (
        <>
          {a.usage.percent !== null && a.usage.percent !== undefined && !a.provider.key_set && (
            <p className="text-body" style={{ color: "var(--ink-dim)" }} data-testid="org-ai-usage">
              {a.usage.percent}% of this month&apos;s allowance is used.
            </p>
          )}
          {a.provider.key_set && (
            <p className="text-body" style={{ color: "var(--ink-dim)" }}>
              Your organisation&apos;s own key is in use, so its model use does not count against the allowance and carries on when the allowance is used up.
              People who set a key of their own in Settings use that one instead.
            </p>
          )}
          <form onSubmit={save} className="flex flex-col gap-3 max-w-[28rem]">
            <h3 className="label">Your organisation&apos;s own provider</h3>
            {field("org-ai-base", "Endpoint (leave empty for OpenAI)", "base_url", { placeholder: "https://api.openai.com/v1", autoComplete: "off" })}
            {field("org-ai-key", a.provider.key_set ? "API key (set; type a new one to replace it)" : "API key", "api_key", { type: "password", autoComplete: "off", placeholder: a.provider.key_set ? "••••••••" : "" })}
            {field("org-ai-chat", "Chat model (optional)", "chat_model", { autoComplete: "off" })}
            {field("org-ai-embedding", "Embedding model (optional)", "embedding_model", { autoComplete: "off" })}
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              The key is stored encrypted and never shown again. Changing the endpoint or the embedding model makes stored search vectors stale: they are rebuilt in the background, using this key.
            </p>
            {error && <ErrorLine error={error} />}
            <div className="flex flex-wrap items-center gap-3">
              <button type="submit" className="btn btn-primary" disabled={setProvider.isPending || (!form && !saved && !draft.api_key)}>
                {setProvider.isPending ? "Saving…" : "Save provider"}
              </button>
              {a.provider.set && (
                <button type="button" className="btn btn-ghost" disabled={removeProvider.isPending} onClick={() => run(() => removeProvider.mutateAsync(), "Could not remove the provider.")}>
                  Remove our own key
                </button>
              )}
              {saved && (
                <span role="status" className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                  Saved.
                </span>
              )}
            </div>
          </form>
          <div className="flex flex-col gap-2 max-w-[28rem]">
            <h3 className="label">Switch AI off</h3>
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              {a.enabled
                ? "Turning AI off stops every call to a language model for this organisation's content, with any key, including a key a member set for themselves. Background work waits and picks up again when you turn it back on."
                : "AI is off: nothing from this organisation is sent to a language model. Features that need one say so; saving, reading and keyword search work as usual."}
            </p>
            <button
              type="button"
              className={a.enabled ? "btn btn-ghost self-start" : "btn btn-primary self-start"}
              disabled={setEnabled.isPending}
              onClick={() => run(() => setEnabled.mutateAsync(!a.enabled), "Could not change the AI switch.")}
            >
              {setEnabled.isPending ? "Saving…" : a.enabled ? "Switch AI off" : "Switch AI on"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
