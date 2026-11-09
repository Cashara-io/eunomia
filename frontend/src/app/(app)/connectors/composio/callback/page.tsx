"use client";
import { useEffect, useState } from "react";
import Link from "next/link";
import { API_URL } from "@/lib/api";

export default function ComposioCallback() {
  const [message, setMessage] = useState("Completing Composio sign-in…");
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const code = params.get("code");
    const state = params.get("state");
    window.history.replaceState(null, "", window.location.pathname);
    if (!code || !state || params.has("error")) { void Promise.resolve().then(() => setMessage("Composio sign-in was not completed. Return to Connected sources to try again.")); return; }
    void fetch(`${API_URL}/api/connectors/composio/callback`, { method: "POST", credentials: "include", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, state }) }).then((response) => {
      if (!response.ok) throw new Error("Sign-in could not be completed. Return to Connected sources and reconnect Composio.");
      window.location.replace("/connectors");
    }).catch((error: unknown) => setMessage(error instanceof Error ? error.message : "Sign-in could not be completed."));
  }, []);
  return <div className="flex flex-col gap-4"><h1 className="page-title">Composio</h1><p role="status">{message}</p><Link className="link" href="/connectors">Connected sources</Link></div>;
}
