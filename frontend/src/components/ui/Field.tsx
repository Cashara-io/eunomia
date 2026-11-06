import { useId } from "react";

/** A labelled form field: the label stays visible above the control, a hint
 * explains what to enter, and an error says how to fix it. The child gets
 * id / aria-describedby / aria-invalid through the render prop. */
export default function Field({
  label,
  hint,
  error,
  required,
  children,
  className = "",
}: {
  label: React.ReactNode;
  hint?: React.ReactNode;
  error?: React.ReactNode;
  required?: boolean;
  children: (props: { id: string; "aria-describedby"?: string; "aria-invalid"?: boolean; required?: boolean }) => React.ReactNode;
  className?: string;
}) {
  const id = useId();
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  return (
    <div className={`flex flex-col gap-1.5 ${className}`}>
      <label htmlFor={id} className="text-[14px] font-medium" style={{ color: "var(--ink)" }}>
        {label}
        {required && <span style={{ color: "var(--ink-dim)" }}> (required)</span>}
      </label>
      {children({ id, "aria-describedby": describedBy, "aria-invalid": error ? true : undefined, required })}
      {hint && (
        <p id={hintId} className="text-[13px] leading-snug" style={{ color: "var(--ink-dim)" }}>
          {hint}
        </p>
      )}
      {error && (
        <p id={errorId} className="text-[13px] leading-snug" style={{ color: "var(--critical)" }}>
          {error}
        </p>
      )}
    </div>
  );
}
