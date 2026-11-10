import { useEffect, useState, type RefObject } from "react";

// This remembers presentation only. Decisions keep their existing activation rules.
export function useReviewSticky(
  region: RefObject<HTMLElement | null>,
  id: string,
  revision: number,
  enabled: boolean,
) {
  const key = JSON.stringify([id, revision]);
  const [seenKey, setSeenKey] = useState<string | null>(null);
  useEffect(() => {
    const article = region.current;
    const bar = article?.querySelector("footer");
    const evidence = article?.querySelector('[aria-label="Source evidence"]');
    const consequence = article?.querySelector('[aria-label="Consequence of approving"]');
    if (!enabled || !article || !bar || !evidence || !consequence) return;
    const seen = new Set<Element>();
    const measure = () => {
      const barHeight = bar.getBoundingClientRect().height;
      article.style.setProperty("--decision-bar-height", `${barHeight}px`);
      // Exclude our own reserved space when checking whether the detail fits.
      const long = article.getBoundingClientRect().height - parseFloat(getComputedStyle(article).paddingBottom) > window.innerHeight;
      setSeenKey(long && seen.size === 2 ? key : null);
    };
    const observer = new IntersectionObserver(entries => {
      for (const entry of entries) if (entry.isIntersecting) seen.add(entry.target);
      measure();
    });
    observer.observe(evidence);
    observer.observe(consequence);
    const size = new ResizeObserver(measure);
    size.observe(bar);
    size.observe(article);
    window.addEventListener("resize", measure);
    measure();
    return () => {
      observer.disconnect();
      size.disconnect();
      window.removeEventListener("resize", measure);
      article.style.removeProperty("--decision-bar-height");
    };
  }, [region, key, enabled]);
  return enabled && seenKey === key;
}
