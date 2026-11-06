import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";
import "blobatar/motion.css";
import Providers from "@/components/Providers";

const geist = Geist({
  variable: "--font-geist",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "Eunomia",
  description: "Shared memory for the AI apps you already use",
};

const DIRECTION_CONTRACT = `
THESIS: Eunomia is shared memory for the AI apps people already use. Make the next step obvious for people who should not need to understand MCP.
OWN-WORLD: Bauhaus Memory. Cream page #FAFAF7, white work surfaces, ink #171717 text and ink pill buttons, blue #3159B6 focus ring. Three flat vault colours with shapes: personal red circle #CF412F, project yellow square #E8BE39, company blue triangle #3159B6. Light by default, warm "ink paper" dark opt-in. Geist. Phosphor Duotone icons with a solid vault-colour back layer. Pill buttons and nav chips, 10px fields, 12px cards.
STORY: Choose your AI app, follow its own steps, sign in and approve, then try a recall. Then bring in knowledge, find it, and control who can see it.
FIRST VIEWPORT: White sidebar with the shape-trio mark and nav chips; Home asks "Which AI app do you use?" with ChatGPT, Claude Code, Codex and Other apps, nothing preselected.
FINISH: status in words, confirmations name target and consequence, success only from real backend state. DESIGN.md is the source of truth.
`;

// Light is the default. A saved dark choice is applied before first paint so
// the page never flashes cream first.
const THEME_SCRIPT = `try{if(localStorage.getItem("eunomia-theme")==="dark")document.documentElement.dataset.theme="dark"}catch(e){}`;

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={`${geist.variable} ${geistMono.variable} h-full antialiased`} suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }} />
      </head>
      <body className="min-h-full flex">
        <div hidden dangerouslySetInnerHTML={{ __html: `<!--${DIRECTION_CONTRACT}-->` }} />
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
