"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { usePeople } from "@/lib/queries/admin";
import { PanelHead, TableSkeleton, relativeTime } from "./shared";

const ADMIN_WORDS: Record<string, string> = {
  creator: "Admin (created the workspace)",
  operator: "Admin (set on the server)",
};

const date = (iso?: string | null) => (iso ? new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "—");

export function PeopleSection() {
  const query = usePeople();
  const data = query.data;
  const people = data?.people ?? null;
  const signup = data?.signup;

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="People">Everyone with an account in this workspace.</PanelHead>

      {query.isError && <ErrorLine error={failure(query.error, "Could not load the people in this workspace. Reload to try again.")} />}

      {signup && (
        <div className="flex flex-col gap-1.5 text-[14px]">
          <div>
            <span className="label mr-2">Who can sign up</span>
            <span>{signup.summary}</span>
          </div>
          {signup.mode === "invite" && (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              {signup.allowlist.length ? `Allowed: ${signup.allowlist.join(", ")}` : "No emails are on the allowlist yet."}
            </p>
          )}
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            {signup.operators.length
              ? `Admins set on the server: ${signup.operators.join(", ")}`
              : "No admins are set on the server; the first account is the admin."}
          </p>
          <details className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
            <summary className="cursor-pointer select-none">How to change this</summary>
            <p className="mt-1 max-w-[68ch]">
              These come from the server&apos;s settings: <code className="font-mono">SIGNUP</code> (open, invite or closed),{" "}
              <code className="font-mono">SIGNUP_ALLOWLIST</code> and <code className="font-mono">EUNOMIA_ADMIN_EMAILS</code>. Change
              them in your <code className="font-mono">.env</code> file and restart Eunomia.
            </p>
          </details>
        </div>
      )}

      <div className="overflow-x-auto">
        <table className="data-table w-full text-[13px]">
          <thead>
            <tr>
              <th className="text-left">Email</th>
              <th className="text-left w-[130px]">Joined</th>
              <th className="text-left w-[110px]">Last active</th>
              <th className="text-right w-[110px]">Shared spaces</th>
            </tr>
          </thead>
          <tbody>
            {people === null ? (
              <TableSkeleton cols={4} />
            ) : people.length === 0 ? (
              <tr>
                <td colSpan={4} className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
                  No accounts yet.
                </td>
              </tr>
            ) : (
              people.map((p) => (
                <tr key={p.id}>
                  <td className="max-w-[320px]">
                    <span className="flex flex-col min-w-0">
                      <span className="truncate" title={p.email}>
                        {p.email}
                      </span>
                      {ADMIN_WORDS[p.admin] && (
                        <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                          {ADMIN_WORDS[p.admin]}
                        </span>
                      )}
                    </span>
                  </td>
                  <td className="whitespace-nowrap" style={{ color: "var(--ink-dim)" }}>
                    {date(p.created_at)}
                  </td>
                  <td
                    className="whitespace-nowrap"
                    style={{ color: "var(--ink-dim)" }}
                    title={p.last_active ? new Date(p.last_active).toLocaleString() : undefined}
                  >
                    {relativeTime(p.last_active ?? null)}
                  </td>
                  <td className="text-right tabular-nums">{p.shared_spaces}</td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
