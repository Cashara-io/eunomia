"use client";

import { useState } from "react";

/** Inline two-step confirm (DESIGN.md): the first click arms it, the second does the thing. No native dialog. */
export default function ConfirmButton({
  label,
  confirmLabel,
  onConfirm,
  className,
  disabled,
  children,
}: {
  /** Accessible name of the first-step button. */
  label: string;
  /** Text of the danger button that confirms. */
  confirmLabel: string;
  onConfirm: () => void;
  className: string;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  const [armed, setArmed] = useState(false);
  if (!armed)
    return (
      <button type="button" onClick={() => setArmed(true)} aria-label={label} disabled={disabled} className={className}>
        {children}
      </button>
    );
  return (
    <span className="inline-flex items-center gap-1.5 shrink-0">
      <button
        type="button"
        autoFocus
        onClick={() => {
          setArmed(false);
          onConfirm();
        }}
        disabled={disabled}
        className="btn btn-danger-solid btn-sm"
      >
        {confirmLabel}
      </button>
      <button type="button" onClick={() => setArmed(false)} aria-label="Keep" className="btn btn-sm">
        Cancel
      </button>
    </span>
  );
}
