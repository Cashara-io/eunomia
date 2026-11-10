import { useEffect } from "react";
import { z } from "zod";
const position = z.object({ y: z.number(), x: z.number(), section: z.string().nullable() });
export function rememberObjects() {
  try {
    const table = document.querySelector('[aria-label="Objects table"]');
    sessionStorage.setItem(`object-position:${location.pathname}${location.search}`, JSON.stringify({ y: window.scrollY, x: table?.scrollLeft ?? 0, section: new URLSearchParams(location.search).get("section") }));
  } catch { /* Navigation works when storage is disabled. */ }
}
export function useObjectReturn(ready: boolean) {
  useEffect(() => {
    if (!ready) return;
    try {
      const stored = position.safeParse(JSON.parse(sessionStorage.getItem(`object-position:${location.pathname}${location.search}`) ?? "null"));
      if (!stored.success) return;
      const frame = requestAnimationFrame(() => {
        const table = document.querySelector('[aria-label="Objects table"]');
        if (table) table.scrollLeft = stored.data.x;
        window.scrollTo(0, stored.data.y);
      });
      return () => cancelAnimationFrame(frame);
    } catch { /* No saved presentation state. */ }
  }, [ready]);
}
