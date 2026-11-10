// Presentation only. Routes, query values and definition keys keep their API spelling.
export function humanLabel(value: string, displayName?: string | null) {
  if (displayName?.trim()) return displayName.trim();
  const text = value
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .trim();
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export function plural(value: string, count = 2) {
  if (count === 1) return value;
  const irregular: Record<string, string> = {
    person: "people",
    child: "children",
    company: "companies",
  };
  if (irregular[value.toLowerCase()]) {
    const word = irregular[value.toLowerCase()];
    return value[0] === value[0]?.toUpperCase() ? humanLabel(word) : word;
  }
  if (/(?:s|x|z|ch|sh)$/i.test(value)) return `${value}es`;
  if (/[^aeiou]y$/i.test(value)) return `${value.slice(0, -1)}ies`;
  return `${value}s`;
}

export function relativeTime(at: string, now: number) {
  const elapsed = Math.max(0, now - new Date(at).getTime());
  if (elapsed < 60_000) return "Just now";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)} min ago`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)} hr ago`;
  return new Date(at).toLocaleTimeString("en-AU", {
    hour: "numeric",
    minute: "2-digit",
  });
}

export function activityDay(at: string, now: number) {
  const date = new Date(at);
  if (date.toDateString() === new Date(now).toDateString()) return "Today";
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString("en-AU", {
    day: "numeric",
    month: "long",
    year: "numeric",
  });
}
