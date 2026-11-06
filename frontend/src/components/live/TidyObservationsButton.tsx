"use client";

import { useState } from "react";
import { Sparkles } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useConsolidateObservations } from "@/lib/queries/entities";

/**
 * "Tidy up": folds new facts about one person or topic into its short summary of what is known (the
 * `consolidate_observations` tool). Uses the configured AI model; says so plainly when there is none.
 */
export default function TidyObservationsButton({ entityId, onDone, disabled }: { entityId: string; onDone?: () => void; disabled?: boolean }) {
  const tidy = useConsolidateObservations();
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  async function run() {
    setMessage(null);
    setError(null);
    try {
      const out = await tidy.mutateAsync(entityId);
      if (out.error) throw new Error(out.error);
      if (out.note) setMessage("Tidying up needs an AI model. Add one in Settings, then try again.");
      else if (out.errors?.length) setError({ message: `Could not tidy up: ${out.errors[0]}` });
      else if (out.consolidated?.length) setMessage("Done. The summary now includes the newest facts.");
      else setMessage("Already tidy. There are no new facts to fold in.");
      onDone?.();
    } catch (e) {
      setError(failure(e, "Could not tidy up these memories."));
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2 flex-wrap">
        <button
          type="button"
          className="btn btn-sm"
          disabled={disabled || tidy.isPending}
          onClick={() => void run()}
          title="Combine new facts into one short summary of what is known"
        >
          <Sparkles size={13} strokeWidth={1.75} aria-hidden />
          {tidy.isPending ? "Tidying up…" : "Tidy up"}
        </button>
        {message && (
          <span className="text-[12.5px]" role="status" style={{ color: "var(--ink-dim)" }}>
            {message}
          </span>
        )}
      </div>
      {error && <ErrorLine error={error} />}
    </div>
  );
}
