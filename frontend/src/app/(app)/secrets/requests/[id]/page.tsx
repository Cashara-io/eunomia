"use client";

import { useState } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { ApiError } from "@/lib/api";
import { useSecretRequest } from "@/lib/queries/secretRequests";
import SecretRequestForm from "@/components/SecretRequestForm";
import ErrorLine, { failure } from "@/components/ErrorLine";

/** `/secrets/requests/<id>`: the URL an agent shows (or an MCP client opens) for one secret request. */
export default function SecretRequestPage() {
  const { id } = useParams<{ id: string }>();
  const request = useSecretRequest(id);
  const [outcome, setOutcome] = useState<"filled" | "declined" | null>(null);
  const notFound = request.error instanceof ApiError && request.error.status === 404;
  return (
    <div className="max-w-[560px] mx-auto flex flex-col gap-4">
      <h1 className="page-title">Secret request</h1>
      {request.isLoading && <span className="skeleton h-4 w-48" aria-hidden />}
      {notFound && (
        <p className="ledger px-4 py-4 text-[13px]" style={{ color: "var(--ink-dim)" }}>
          No such request for you. It may be for someone else, or the link is wrong. Ask the agent to make a new request.
        </p>
      )}
      {request.isError && !notFound && <ErrorLine error={failure(request.error, "Could not load the request.", " Reload the page to try again.")} />}
      {outcome ? (
        <div className="ledger p-5 flex flex-col gap-2" role="status">
          <div className="text-[14px] font-medium">{outcome === "filled" ? "Saved" : "Declined"}</div>
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            {outcome === "filled" ? "The agent can now use it without ever seeing it. You can close this tab." : "The agent will be told you declined."}
          </p>
          <Link href="/connectors" className="text-[13px] underline">
            Manage held secrets
          </Link>
        </div>
      ) : (
        request.data && (
          <div className="ledger p-5">
            <SecretRequestForm request={request.data} onDone={setOutcome} />
          </div>
        )
      )}
    </div>
  );
}
