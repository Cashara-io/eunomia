"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useUpdateConnector } from "@/lib/queries/connectors";
import type { Connector } from "@/lib/types";

/**
 * "Keep audio recordings" for a Pocket connection (#117): sets `config.store_audio`. Off by default.
 * Saved as soon as it is ticked.
 */
export default function PocketAudioSetting({ connector }: { connector: Connector }) {
  const update = useUpdateConnector();
  const on = connector.config?.store_audio === true;
  const [state, setState] = useState<"idle" | "saved">("idle");
  const [error, setError] = useState<Failure | null>(null);

  async function change(next: boolean) {
    setError(null);
    setState("idle");
    try {
      await update.mutateAsync({ connector: connector.id, body: { config: { store_audio: next } } });
      setState("saved");
    } catch (e) {
      setError(failure(e, "Could not save this setting."));
    }
  }

  return (
    <section className="flex flex-col gap-2 pt-6 border-t" aria-labelledby="pocket-audio-heading">
      <h2 id="pocket-audio-heading" className="section-title">
        Audio recordings
      </h2>
      <label className="flex items-start gap-3 text-[14px] cursor-pointer">
        <input
          type="checkbox"
          className="mt-1 w-4 h-4 shrink-0"
          style={{ accentColor: "var(--accent)" }}
          checked={on}
          disabled={update.isPending}
          onChange={(e) => void change(e.target.checked)}
        />
        <span className="flex flex-col gap-0.5">
          <span className="font-medium">Keep audio recordings</span>
          <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Saves a copy of each recording&apos;s sound next to its transcript, so you can play it back here. This uses storage space.
            Audio only arrives once your Pocket device has uploaded it, so it can appear a little after the transcript. Recordings longer
            than about 200 MB keep their transcript only.
          </span>
        </span>
      </label>
      <span className="text-[13px]" role="status" style={{ color: "var(--good)" }}>
        {state === "saved" ? (on ? "Saved: new recordings will keep their audio." : "Saved: audio will not be kept.") : ""}
      </span>
      {error && <ErrorLine error={error} />}
    </section>
  );
}
