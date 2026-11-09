import { Fragment, type ReactNode } from "react";
import type { ChatNode } from "@/lib/gen/types.gen";

// React escapes every text child and attribute. Only safe navigation protocols
// reach an anchor; server hrefs are never treated as Markdown or prose.
function linkHref(href: string): string | undefined {
  const protocol = href.replace(/[\u0000-\u0020\u007f]/g, "").match(/^([a-z][a-z0-9+.-]*):/i)?.[1];
  return !protocol || /^(https?|mailto)$/i.test(protocol) ? href : undefined;
}

function renderNode(node: ChatNode): ReactNode {
  const children = () => "children" in node ? renderNodes(node.children) : null;
  switch (node.kind) {
    case "text": return node.text;
    case "paragraph": return <p>{children()}</p>;
    case "heading": {
      switch (node.level) {
        case 1: return <h1>{children()}</h1>;
        case 2: return <h2>{children()}</h2>;
        case 3: return <h3>{children()}</h3>;
        case 4: return <h4>{children()}</h4>;
        case 5: return <h5>{children()}</h5>;
        default: return <h6>{children()}</h6>;
      }
    }
    case "list": return node.start == null ? <ul>{children()}</ul> : <ol start={node.start}>{children()}</ol>;
    case "item": return <li>{children()}</li>;
    case "table": return <table>{node.children.filter(n => n.kind === "table_head").map((n, i) => <Fragment key={i}>{renderNode(n)}</Fragment>)}<tbody>{renderNodes(node.children.filter(n => n.kind !== "table_head"))}</tbody></table>;
    case "table_head": return <thead><tr>{node.children.map((n, i) => <th key={i}>{"children" in n ? renderNodes(n.children) : renderNode(n)}</th>)}</tr></thead>;
    case "table_row": return <tr>{children()}</tr>;
    case "table_cell": return <td>{children()}</td>;
    case "strong": return <strong>{children()}</strong>;
    case "em": return <em>{children()}</em>;
    case "code_span": return <code>{children()}</code>;
    case "code_block": return <pre><code>{children()}</code></pre>;
    case "link": return <a href={linkHref(node.href)} title={node.title || undefined} target="_blank" rel="noopener noreferrer">{children()}</a>;
    case "line_break": return <><br />{children()}</>;
    default: {
      const exhaustive: never = node;
      return exhaustive;
    }
  }
}

function renderNodes(nodes: ChatNode[]): ReactNode {
  return nodes.map((node, i) => <Fragment key={i}>{renderNode(node)}</Fragment>);
}

/** Assistant replies are already parsed and checked by the server. */
export default function ChatText({ tree }: { tree: ChatNode[] }) {
  return <div className="md">{renderNodes(tree)}</div>;
}
