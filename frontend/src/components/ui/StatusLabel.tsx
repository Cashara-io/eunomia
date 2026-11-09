import MorphText from "./MorphText";
export type StatusTone = "good" | "warning" | "critical" | "info" | "neutral";

/** Status always in words: a small shape supports the text, never replaces it. */
export default function StatusLabel({ tone, children, className = "" }: { tone: StatusTone; children: React.ReactNode; className?: string }) {
  return (
    <span className={`status-label ${className}`} data-tone={tone}>
      <span className="status-shape" aria-hidden />
      <span>{typeof children === "string" ? <MorphText>{children}</MorphText> : children}</span>
    </span>
  );
}

/** Maps the tone strings older code passes around (CSS variables) to a StatusLabel tone. */
export function toneFromVar(v: string): StatusTone {
  if (v.includes("good")) return "good";
  if (v.includes("warning")) return "warning";
  if (v.includes("critical")) return "critical";
  if (v.includes("accent")) return "info";
  return "neutral";
}
