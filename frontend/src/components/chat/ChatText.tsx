/** The server has already stripped Markdown and checked the visible text. */
export default function ChatText({ text }: { text: string }) {
  return <span className="whitespace-pre-wrap">{text}</span>;
}
