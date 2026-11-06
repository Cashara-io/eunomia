"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useConsolidateObservations } from "@/lib/queries/entities";

/**
 * "Summarise memories": the AI model writes one short statement of what is known about this person
 * or topic from its separate memories, or brings that statement up to date with the newest ones (the
 * `consolidate_observations` tool). Nothing is merged or deleted. Says so plainly when there is no model.
 * (Was "Tidy up", which sounded like it removed things.)
 */
export default function TidyUp({ entityId, onDone, disabled }: { entityId: string; onDone?: () => void; disabled?: boolean }) {
  const tidy = useConsolidateObservations();
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  async function run() {
    setMessage(null);
    setError(null);
    try {
      const out = await tidy.mutateAsync(entityId);
      if (out.error) throw new Error(out.error);
      if (out.note) setMessage("Summarising needs an AI model. Add one in Settings, then try again.");
      else if (out.errors?.length) setError({ message: `Could not summarise: ${out.errors[0]}` });
      else if (out.consolidated?.length) setMessage("Done. The summary now includes the newest memories.");
      else setMessage("Already up to date. No new memories to add to the summary.");
      onDone?.();
    } catch (e) {
      setError(failure(e, "Could not summarise these memories."));
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2 flex-wrap">
        <button
          type="button"
          className="text-[13px] underline-offset-2 hover:underline disabled:opacity-50 disabled:no-underline"
          style={{ color: "var(--accent-text)" }}
          disabled={disabled || tidy.isPending}
          onClick={() => void run()}
          title="Write one short summary of what these memories say. Nothing is deleted."
        >
          {tidy.isPending ? "Summarising…" : "Summarise memories"}
        </button>
        {message && (
          <span className="text-[13px]" role="status" style={{ color: "var(--ink-dim)" }}>
            {message}
          </span>
        )}
      </div>
      {error && <ErrorLine error={error} />}
    </div>
  );
}
