import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Plugin } from "unified";
import type {} from "remark-parse";

// Footnote counters are generated numbers, outside the prose evidence contract.
const commonMarkFootnotes: Plugin = function () {
  const data = this.data();
  const extensions = data.micromarkExtensions || (data.micromarkExtensions = []);
  extensions.push({ disable: { null: ["gfmFootnoteDefinition", "gfmFootnoteCall", "gfmPotentialFootnoteCall"] } });
};

/** CommonMark plus GFM tables, strikethrough and tasks, matching server checks. */
export default function ChatText({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, commonMarkFootnotes]}
        skipHtml
        components={{ a: ({ children, href, title }) => <a href={href} title={title} target="_blank" rel="noopener noreferrer">{children}</a> }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
