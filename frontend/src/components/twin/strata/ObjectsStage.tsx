"use client";
import { CaretDownIcon, StackIcon } from "@phosphor-icons/react";
import { useKindColor } from "@/lib/kindColors";
import type { ConnectionNode } from "@/lib/queries/connections";
import type { ObjectProfile } from "@/lib/queries/objects";
import ConnectionKindIcon from "../ConnectionKindIcon";
import { formatted } from "../value-format";
import { humanLabel, plural } from "../overview-format";
import { LANE_OPEN_LIMIT, laneOfNode, placeLanes, type Lane } from "./strata-model";
import type { Wire } from "./Wires";
import styles from "./strata.module.css";

/** Lines of the Objects level: focus to lane, parent lane to deeper lane, definitions to the selected lane, node to inspector. */
export function objectsWires(lanes: Lane[], focus: string, focusKind: string, selected: string, tether: boolean): Wire[] {
  const selectedLane = selected && selected !== focus ? laneOfNode(lanes, selected) : undefined;
  const wires: Wire[] = [];
  const placed = placeLanes(lanes);
  for (const lane of lanes) {
    const on = !selected || selected === focus ? false : lane.nodes.some((n) => n.id === selected);
    const tone = on ? "on" : selected && selected !== focus ? "off" : "line";
    if (lane.hop === 1) {
      const side = placed.left.includes(lane) ? "left" : placed.right.includes(lane) ? "right" : "below";
      const down = side === "below";
      wires.push({
        id: `lane:${lane.id}`,
        from: lane.dir === "in" ? `lane:${lane.id}` : "focus",
        to: lane.dir === "in" ? "focus" : `lane:${lane.id}`,
        fromSide: lane.dir === "in" ? (down ? "t" : "r") : down ? "b" : "r",
        toSide: lane.dir === "in" ? (down ? "b" : "l") : down ? "t" : "l",
        tone,
        arrow: true,
      });
    } else if (lane.via)
      wires.push({ id: `via:${lane.id}`, from: `lane:${lane.via}`, to: `lane:${lane.id}`, fromSide: "b", toSide: "t", tone, arrow: true });
  }
  const defKind = selectedLane?.kind ?? focusKind;
  const target = selectedLane ? `lane:${selectedLane.id}` : "focus";
  wires.push({ id: "definition", from: `def:${defKind}`, to: target, fromSide: "b", toSide: "t", tone: "definition" });
  if (tether && selected)
    wires.push({ id: "tether", from: `node:${selected}`, to: "inspector", fromSide: "r", toSide: "l", tone: "tether" });
  return wires;
}

function NodeButton({ node, selected, select, proposed }: { node: ConnectionNode; selected: boolean; select: (id: string) => void; proposed?: boolean }) {
  const color = useKindColor();
  return (
    <button
      className={styles.node}
      data-object={node.id}
      data-wire={`node:${node.id}`}
      data-proposed={proposed}
      aria-pressed={selected}
      onClick={() => select(node.id)}
    >
      <span className={styles.nodeIcon} style={{ color: color(node.kind) }}>
        <ConnectionKindIcon kind={node.kind} />
      </span>
      <span className={styles.nodeText}>
        {node.name}
        <small>{humanLabel(node.kind)}</small>
      </span>
    </button>
  );
}

export default function ObjectsStage({
  focusNode,
  focusProfile,
  lanes,
  selected,
  open,
  mobile,
  select,
  toggle,
  setKind,
}: {
  focusNode: ConnectionNode;
  focusProfile?: ObjectProfile;
  lanes: Lane[];
  selected: string;
  open: string;
  mobile: boolean;
  select: (id: string) => void;
  toggle: (laneId: string) => void;
  setKind: (kind: string) => void;
}) {
  const color = useKindColor();
  const kinds = [...new Set([focusNode.kind, ...lanes.map((l) => l.kind)])];
  const selectedLane = selected && selected !== focusNode.id ? laneOfNode(lanes, selected) : undefined;
  const headline = (focusProfile?.properties ?? []).filter((p) => p.state !== "missing" && p.state !== "unavailable").slice(0, 2);
  const lane = (l: Lane) => {
    const stack = l.nodes.length > (mobile ? 1 : LANE_OPEN_LIMIT);
    const holdsSelection = l.nodes.some((n) => n.id === selected);
    const opened = open === l.id || holdsSelection;
    const dim = !!selected && selected !== focusNode.id && !holdsSelection;
    return (
      <div className={styles.lane} key={l.id} data-lane={l.id} data-dim={dim} data-hop={l.hop}>
        <span className={styles.verb} data-wire={`lane:${l.id}`}>
          {l.hop === 1
            ? l.dir === "in"
              ? `${l.verb} \u2192 focus`
              : `focus ${l.verb} \u2192`
            : `${l.verb}, ${l.hop} hops out`}
        </span>
        {stack ? (
          <>
            <button
              className={styles.stack}
              aria-expanded={opened}
              onClick={() => toggle(l.id)}
              data-wire={`stack:${l.id}`}
            >
              <StackIcon size={16} aria-hidden style={{ color: color(l.kind) }} />
              <span className={styles.nodeText}>
                {l.nodes.length.toLocaleString("en-AU")} {plural(humanLabel(l.kind).toLowerCase(), l.nodes.length)}
                <small>{opened ? "Close the list" : mobile ? "Tap to list" : `Open ${l.nodes.length} loaded`}</small>
              </span>
              <CaretDownIcon size={14} aria-hidden data-open={opened} className={styles.caret} />
            </button>
            {opened && (
              <div className={styles.stackList} role="group" aria-label={`${l.nodes.length} ${humanLabel(l.kind)} ${l.verb}`}>
                {l.nodes.map((n) => (
                  <NodeButton key={n.id} node={n} selected={selected === n.id} select={select} />
                ))}
              </div>
            )}
          </>
        ) : (
          l.nodes.map((n) => <NodeButton key={n.id} node={n} selected={selected === n.id} select={select} />)
        )}
      </div>
    );
  };
  const { left, right, below, far } = placeLanes(lanes);
  const deepest = Math.max(1, ...lanes.map((l) => l.hop));
  return (
    <div className={styles.objects}>
      <div className={styles.definitions} role="group" aria-label="Kinds present around this object">
        <span className={styles.planeLabel}>Definitions</span>
        {kinds.map((k) => (
          <button key={k} className={styles.kindChip} data-wire={`def:${k}`} data-on={k === (selectedLane?.kind ?? focusNode.kind)} onClick={() => setKind(k)}>
            <span className={styles.swatch} style={{ background: color(k) }} aria-hidden />
            {humanLabel(k)}
          </button>
        ))}
      </div>
      <p className={styles.planeNote}>
        Objects, {deepest} {deepest === 1 ? "hop" : "hops"} from {focusNode.name}
      </p>
      <div className={styles.arena}>
        <div className={styles.side} data-side="in">
          {left.map(lane)}
        </div>
        <button
          className={styles.focus}
          data-object={focusNode.id}
          data-wire="focus"
          aria-pressed={selected === focusNode.id}
          onClick={() => select(focusNode.id)}
        >
          <small>
            <span className={styles.swatch} style={{ background: color(focusNode.kind) }} aria-hidden />
            {humanLabel(focusNode.kind)}, focus
          </small>
          <strong>{focusNode.name}</strong>
          {headline.length > 0 && (
            <span className={styles.headlineValues}>
              {headline.map((p) => (
                <span key={p.name} data-state={p.state}>
                  <b className="font-mono">{formatted(p.value, p)}</b>
                  {humanLabel(p.name)}
                  {p.state === "stale" ? ", stale" : ""}
                </span>
              ))}
            </span>
          )}
        </button>
        <div className={styles.side} data-side="out">
          {right.map(lane)}
        </div>
      </div>
      {below.length > 0 && <div className={styles.below}>{below.map(lane)}</div>}
      {far.length > 0 && <div className={styles.far}>{far.map(lane)}</div>}
    </div>
  );
}
