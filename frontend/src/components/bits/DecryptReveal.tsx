// Was a scramble-then-reveal effect for secrets; now the text as is (issue #153).
export default function DecryptReveal({ text }: { text: string; maxMs?: number }) {
  return <>{text}</>;
}
