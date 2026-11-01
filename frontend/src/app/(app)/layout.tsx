import AuthGuard from "@/components/AuthGuard";
import CommandPalette from "@/components/CommandPalette";
import SecretRequestPopup from "@/components/SecretRequestPopup";
import Sidebar from "@/components/Sidebar";
import { InputModeTracker } from "@/components/bits/motion";

export default function AppLayout({ children }: { children: React.ReactNode }) {
  return (
    <AuthGuard>
      <InputModeTracker />
      <div className="flex flex-col md:flex-row flex-1 min-w-0">
        <Sidebar />
        <main className="canvas-grid flex-1 min-w-0 px-4 py-6 md:px-10 md:py-8">{children}</main>
      </div>
      <CommandPalette />
      <SecretRequestPopup />
    </AuthGuard>
  );
}
