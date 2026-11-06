// Was an animated blur-in heading; now renders its text as is. Kept so callers keep working:
// decorative entrance motion read as gimmicky (issue #153).
type Tag = "h1" | "h2" | "h3" | "p" | "span" | "div";

export default function BlurWords({ text, as: As = "span", className }: { text: string; as?: Tag; className?: string }) {
  return <As className={className}>{text}</As>;
}
