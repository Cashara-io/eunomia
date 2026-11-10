import LandscapeSolid from "./LandscapeSolid";
import { memo, type CSSProperties } from "react";
import type { KindItem } from "@/lib/queries/ontology";
import type { overviewLayout } from "./overview-layout";
import { linkSentence } from "./overview-format";
import styles from "./overview.module.css";

type Props = {
  id: string;
  layout: ReturnType<typeof overviewLayout>;
  kinds: KindItem[];
  unlinkedArea: {
    left: number;
    right: number;
    top: number;
    bottom: number;
  } | null;
  tint: (kind: string) => CSSProperties;
};
export default memo(function LandscapePlane({
  id,
  layout,
  kinds,
  unlinkedArea,
  tint,
}: Props) {
  const maxLinks = Math.max(1, ...layout.edges.map((e) => e.link.count));
  const linkScale = (count: number) => (count / maxLinks) * 100;
  return (
    <svg
      className={styles.plane}
      viewBox={`0 0 1200 ${layout.height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={`${id}-fade`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="white" stopOpacity=".12" />
          <stop offset="75%" stopColor="white" stopOpacity=".8" />
        </linearGradient>
        <mask id={`${id}-ground`}>
          <rect width="1200" height={layout.height} fill={`url(#${id}-fade)`} />
        </mask>
        <mask id={`${id}-labels`}>
          <rect width="1200" height={layout.height} fill="white" />
          {layout.nodes.map((n) => (
            <rect
              key={n.kind.kind}
              x={n.x - 116}
              y={n.y - n.height - 90}
              width="232"
              height="78"
              rx="10"
              fill="black"
            />
          ))}
        </mask>
        <radialGradient id={`${id}-shadow`}>
          <stop stopColor="black" stopOpacity=".32" />
          <stop offset="1" stopColor="black" stopOpacity="0" />
        </radialGradient>
        <pattern
          id={`${id}-grid`}
          width="42"
          height="42"
          patternTransform="matrix(1 -.05 .36 .62 0 0)"
          patternUnits="userSpaceOnUse"
        >
          <path
            d="M42 0H0V42"
            fill="none"
            stroke="var(--border-strong)"
            strokeWidth="1"
          />
        </pattern>
        <marker
          id={`${id}-arrow`}
          viewBox="0 0 8 8"
          refX="7"
          refY="4"
          markerWidth="5"
          markerHeight="5"
          orient="auto-start-reverse"
        >
          <path
            d="M1 1 7 4 1 7"
            fill="none"
            stroke="context-stroke"
            strokeWidth="1.5"
          />
        </marker>
      </defs>
      <path
        d={`M22 120 1110 54 1180 ${layout.height - 80} 90 ${layout.height - 14}Z`}
        className={styles.ground}
        fill={`url(#${id}-grid)`}
        mask={`url(#${id}-ground)`}
        stroke="var(--border-strong)"
        strokeWidth="1"
      />
      {unlinkedArea && (
        <g>
          <path
            d={`M${unlinkedArea.left} ${unlinkedArea.top}L${unlinkedArea.right} ${unlinkedArea.top - 18}V${unlinkedArea.bottom}L${unlinkedArea.left} ${unlinkedArea.bottom + 18}Z`}
            fill="none"
            stroke="var(--border-field)"
            strokeDasharray="4 5"
          />
          <text
            x={unlinkedArea.left}
            y={unlinkedArea.bottom + 43}
            fill="var(--ink-dim)"
            fontSize="var(--type-body)"
          >
            Not linked yet
          </text>
        </g>
      )}
      <g mask={`url(#${id}-labels)`}>
        {layout.edges.map((edge) => (
          <LandscapeRoute
            key={edge.key}
            edge={edge}
            id={id}
            width={1 + linkScale(edge.link.count) / 110}
            tint={tint(edge.link.from_kind)}
          />
        ))}
      </g>
      {layout.nodes.map(({ kind, x, y, height, linked }) => (
        <g
          key={kind.kind}
          className={styles.form}
          data-form={`k${kinds.findIndex((k) => k.kind === kind.kind)}`}
          data-unlinked={!linked}
          data-selected="false"
          style={tint(kind.kind)}
        >
          <LandscapeSolid x={x} y={y} height={height} shadow={`${id}-shadow`} />
        </g>
      ))}
    </svg>
  );
});

const LandscapeRoute = memo(function LandscapeRoute({
  edge,
  id,
  width,
  tint,
}: {
  edge: Props["layout"]["edges"][number];
  id: string;
  width: number;
  tint: CSSProperties;
}) {
  const { link, path, token, idle } = edge;
  return (
    <path
      className={styles.route}
      data-route={token}
      data-idle={idle}
      data-from={link.from_kind}
      data-to={link.to_kind}
      d={path}
      fill="none"
      stroke="var(--ink-dim)"
      strokeWidth={width}
      markerEnd={`url(#${id}-arrow)`}
      style={tint}
    >
      <title>{linkSentence(link)}</title>
    </path>
  );
});
