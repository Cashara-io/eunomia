export function humanLabel(value: string) {
  return value.replaceAll("_", " ")
    .replace(/^./, (letter) => letter.toUpperCase());
}

export function relativeTime(at: string, now = Date.now()) {
  const seconds = Math.round((new Date(at).getTime() - now) / 1000);
  const format = new Intl.RelativeTimeFormat("en-AU", { numeric: "auto" });
  for (const [unit, size] of [
    ["year", 31_536_000],
    ["month", 2_592_000],
    ["day", 86_400],
    ["hour", 3600],
    ["minute", 60],
  ] as const) {
    if (Math.abs(seconds) >= size) return format.format(Math.round(seconds / size), unit);
  }
  return Math.abs(seconds) < 10 ? "Just now" : format.format(seconds, "second");
}

export function ReviewTime({ at }: { at: string }) {
  return (
    <time
      dateTime={at}
      title={new Date(at).toLocaleString("en-AU", { timeZoneName: "short" })}
    >
      {relativeTime(at)}
    </time>
  );
}
