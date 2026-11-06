"use client";

import { useEffect, useId, useRef, useState } from "react";

/** A modal confirmation (spec 16.x): names the target in the title, explains
 * the consequence, and offers a clear Cancel. Danger styling only when the
 * action destroys something. Built on <dialog>: focus is trapped while open
 * and returns to the control that opened it. `typeToConfirm` asks the person
 * to type the target's name before an irreversible action unlocks. */
export default function ConfirmDialog({
  open,
  title,
  children,
  confirmLabel,
  cancelLabel = "Cancel",
  danger = false,
  busy = false,
  typeToConfirm,
  error,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: React.ReactNode;
  /** The consequence, in plain words. */
  children: React.ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  danger?: boolean;
  busy?: boolean;
  typeToConfirm?: string;
  error?: React.ReactNode;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const opener = useRef<Element | null>(null);
  const [typed, setTyped] = useState("");
  const titleId = useId();
  const bodyId = useId();
  const inputId = useId();

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

  // The typed name is cleared whenever the dialog is answered, so it never carries over.
  const cancel = () => {
    setTyped("");
    onCancel();
  };
  const confirm = () => {
    setTyped("");
    onConfirm();
  };
  const locked = typeToConfirm !== undefined && typed !== typeToConfirm;

  return (
    <dialog
      ref={ref}
      className="bauhaus-dialog m-auto"
      aria-labelledby={titleId}
      aria-describedby={bodyId}
      onCancel={(e) => {
        e.preventDefault();
        if (!busy) cancel();
      }}
    >
      <form
        method="dialog"
        className="flex flex-col gap-4 p-6"
        onSubmit={(e) => {
          e.preventDefault();
          if (!locked && !busy) confirm();
        }}
      >
        <h2 id={titleId} className="text-[20px] font-medium leading-tight tracking-[-0.025em]">
          {title}
        </h2>
        <div id={bodyId} className="text-[14px] leading-relaxed flex flex-col gap-2" style={{ color: "var(--ink)" }}>
          {children}
        </div>
        {typeToConfirm !== undefined && (
          <div className="flex flex-col gap-1.5">
            <label htmlFor={inputId} className="text-[14px] font-medium">
              Type <span className="font-mono">{typeToConfirm}</span> to confirm
            </label>
            <input
              id={inputId}
              className="field h-10 px-3 text-[14px]"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          </div>
        )}
        {error && (
          <p role="alert" className="text-[13px]" style={{ color: "var(--critical)" }}>
            {error}
          </p>
        )}
        <div className="flex flex-wrap justify-end gap-2 pt-1">
          <button type="button" className="btn" onClick={cancel} disabled={busy} autoFocus>
            {cancelLabel}
          </button>
          <button type="submit" className={`btn ${danger ? "btn-danger-solid" : "btn-primary"}`} disabled={locked || busy} aria-busy={busy}>
            {busy ? "Working…" : confirmLabel}
          </button>
        </div>
      </form>
    </dialog>
  );
}
