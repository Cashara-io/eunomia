// Was a rolling-digit counter; now the number as text (issue #153: decorative motion removed).
const defaultFormat = (n: number) => n.toLocaleString();

export default function DigitRoll({ value, format = defaultFormat }: { value: number; format?: (n: number) => string }) {
  return <>{format(value)}</>;
}
