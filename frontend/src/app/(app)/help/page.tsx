import { redirect } from "next/navigation";

/** /help is what people type; the guide lives at /docs. */
export default function HelpPage() {
  redirect("/docs");
}
