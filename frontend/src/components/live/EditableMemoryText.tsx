"use client";

import { PencilSimpleIcon } from "@phosphor-icons/react/ssr";
import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useUpdateMemory } from "@/lib/queries/entities";
import type { EntityMemory } from "@/lib/types";

/** A memory's text with an "Edit" action (the same change an AI tool makes with `memory_update`). */
export default function EditableMemoryText({ memory, onSaved, disabled }: { memory: EntityMemory; onSaved?: () => void; disabled?: boolean }) {
  const update = useUpdateMemory();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(memory.text);
  const [error, setError] = useState<Failure | null>(null);

  if (!editing) {
    return (
      <span className="text-[14px] leading-normal">
        {memory.text}{" "}
        <button
          type="button"
          className="btn btn-ghost btn-sm btn-icon align-middle w-[22px] h-[22px] opacity-70 hover:opacity-100 focus-visible:opacity-100"
          aria-label="Edit memory"
          title="Edit memory"
          disabled={disabled}
          onClick={() => {
            setDraft(memory.text);
            setError(null);
            setEditing(true);
          }}
        >
          <PencilSimpleIcon weight="duotone" size={12} aria-hidden />
        </button>
      </span>
    );
  }

  const text = draft.trim();
  async function save(e: React.FormEvent) {
    e.preventDefault();
    if (!text || text === memory.text) return setEditing(false);
    setError(null);
    try {
      await update.mutateAsync({ memoryId: memory.id, text });
      setEditing(false);
      onSaved?.();
    } catch (err) {
      setError(failure(err, "Could not save that change."));
    }
  }

  return (
    <form onSubmit={save} className="flex flex-col gap-2 w-full">
      <textarea
        className="field px-3 py-2 text-[14px]"
        rows={3}
        aria-label="Memory text"
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") setEditing(false);
        }}
      />
      <div className="flex gap-2">
        <button type="submit" className="btn btn-primary btn-sm" disabled={update.isPending || !text}>
          {update.isPending ? "Saving…" : "Save"}
        </button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => setEditing(false)}>
          Cancel
        </button>
      </div>
      {error && <ErrorLine error={error} />}
    </form>
  );
}
