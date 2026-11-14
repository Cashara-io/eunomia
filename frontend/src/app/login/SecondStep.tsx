"use client";

import { useCallback, useState } from "react";
import { ArrowRight, Loader2 } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { CodeField, EnrolSteps, RecoveryCodes, type Enrolment } from "@/components/mfa/Mfa";
import { ExistingAccess } from "@/components/mfa/ExistingAccess";
import { useConfirmChallengeEnrolment, useStartChallengeEnrolment, useVerifyChallenge } from "@/lib/queries/mfa";

const EXPIRED = "auth.mfa_challenge_expired";

/** The second sign-in step after a correct password: a code (`verify`), or setting up an authenticator
 * first when the server requires it (`enrol`). `onRestart` goes back to the password with a message;
 * `onDone` continues into the app. */
export default function SecondStep({
  step,
  onDone,
  onRestart,
}: {
  step: "verify" | "enrol";
  onDone: () => void;
  onRestart: (why: Failure) => void;
}) {
  return step === "enrol" ? <EnrolStep onDone={onDone} onRestart={onRestart} /> : <VerifyStep onDone={onDone} onRestart={onRestart} />;
}

function VerifyStep({ onDone, onRestart }: { onDone: () => void; onRestart: (why: Failure) => void }) {
  const verify = useVerifyChallenge();
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await verify.mutateAsync(code.trim());
      onDone();
    } catch (err) {
      const f = failure(err, "That code didn't work. Try again.");
      if (f.code === EXPIRED) return onRestart(f);
      setError(f);
      setCode("");
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <div className="mb-3">
        <h1 className="connect-title">Two-step verification</h1>
        <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
          {recovery ? "Enter one of the recovery codes you saved. Each works once." : "Enter the six-digit code from your authenticator app."}
        </p>
      </div>
      <CodeField
        key={recovery ? "recovery" : "totp"}
        id="mfa-code"
        label={recovery ? "Recovery code" : "Code"}
        recovery={recovery}
        value={code}
        onChange={setCode}
        invalid={Boolean(error)}
      />
      {error && <ErrorLine error={error} />}
      <button type="submit" disabled={busy || !code.trim()} aria-busy={busy} className="btn btn-primary mt-1 min-h-11 w-full">
        {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
        {busy ? "Checking…" : "Verify"}
        {!busy && <ArrowRight size={16} strokeWidth={1.75} aria-hidden />}
      </button>
      <button
        type="button"
        className="link text-body font-medium self-start"
        onClick={() => {
          setRecovery((r) => !r);
          setCode("");
          setError(null);
        }}
      >
        {recovery ? "Use your authenticator app instead" : "Lost your authenticator? Use a recovery code"}
      </button>
    </form>
  );
}

function EnrolStep({ onDone, onRestart }: { onDone: () => void; onRestart: (why: Failure) => void }) {
  const start = useStartChallengeEnrolment();
  const confirm = useConfirmChallengeEnrolment();
  const [codes, setCodes] = useState<string[] | null>(null);
  const startEnrolment = start.mutateAsync;
  const confirmEnrolment = confirm.mutateAsync;

  const begin = useCallback(async (): Promise<Enrolment> => {
    try {
      return await startEnrolment();
    } catch (e) {
      const f = failure(e, "Could not start the setup.");
      if (f.code === EXPIRED) onRestart(f);
      throw e;
    }
  }, [startEnrolment, onRestart]);

  const finish = useCallback(
    async (code: string) => {
      try {
        return (await confirmEnrolment(code)).recovery_codes;
      } catch (e) {
        const f = failure(e, "That code didn't work.");
        if (f.code === EXPIRED) onRestart(f);
        throw e;
      }
    },
    [confirmEnrolment, onRestart],
  );

  return (
    <div className="flex flex-col gap-4">
      <div className="mb-3">
        <h1 className="connect-title">{codes ? "Save your recovery codes" : "Set up two-step verification"}</h1>
        {!codes && (
          <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
            This server requires a code from an authenticator app each time you sign in with your password. Set it up now to continue.
          </p>
        )}
      </div>
      {codes ? <RecoveryCodes codes={codes} onDone={onDone} doneLabel="Continue"><ExistingAccess /></RecoveryCodes> : <EnrolSteps start={begin} confirm={finish} onCodes={setCodes} />}
    </div>
  );
}
