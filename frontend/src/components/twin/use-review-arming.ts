"use client";

import { useLayoutEffect, useRef, type RefObject } from "react";
import { usePathname, useSearchParams } from "next/navigation";

function routeKey(pathname: string, search: string) {
  return JSON.stringify([pathname, new URLSearchParams(search).toString()]);
}

type ArmedReview = {
  id: string;
  revision: number;
  route: string;
  mode: string;
};

function clearArm(
  armed: RefObject<ArmedReview | null>,
  region: RefObject<HTMLElement | null>,
  heading: RefObject<HTMLHeadingElement | null>,
) {
  armed.current = null;
  if (region.current?.contains(document.activeElement) &&
      document.activeElement?.hasAttribute("data-review-decision")) {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    heading.current?.focus({ preventScroll: true });
  }
}

export function useReviewArming({
  id, revision, incomingId, incomingRevision, enabled, mode, region, heading,
}: {
  id: string;
  revision: number;
  incomingId: string;
  incomingRevision: number;
  enabled: boolean;
  mode: "decision" | "describe" | "merge";
  region: RefObject<HTMLElement | null>;
  heading: RefObject<HTMLHeadingElement | null>;
}) {
  const pathname = usePathname();
  const params = useSearchParams();
  const route = routeKey(pathname, params.toString());
  const armed = useRef<ArmedReview | null>(null);
  const transitioning = useRef(false);

  function disarm() {
    clearArm(armed, region, heading);
  }

  useLayoutEffect(() => {
    transitioning.current = false;
  }, [route]);
  useLayoutEffect(() => {
    clearArm(armed, region, heading);
    return () => { armed.current = null; };
  }, [id, revision, incomingId, incomingRevision, enabled, mode, route, region, heading]);

  function available() {
    return enabled && !transitioning.current && id === incomingId &&
      revision === incomingRevision &&
      route === routeKey(window.location.pathname, window.location.search);
  }

  return {
    available,
    hasShortcut: () => armed.current !== null,
    disarm,
    beginNavigation() {
      transitioning.current = true;
      disarm();
    },
    arm() {
      armed.current = available() ? { id, revision, route, mode } : null;
    },
    consume() {
      const seen = armed.current;
      armed.current = null;
      return available() && seen?.id === id && seen.revision === revision &&
        seen.route === route && seen.mode === mode;
    },
  };
}
