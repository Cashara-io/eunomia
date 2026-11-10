import { ApiError } from "@/lib/api";
import ErrorLine from "@/components/ErrorLine";

export default function ReadState({
  error,
  retry,
  subject = "this view",
}: {
  error: unknown;
  retry: () => void;
  subject?: string;
}) {
  const denied = error instanceof ApiError && [403, 404].includes(error.status);
  return (
    <ErrorLine
      action={
        denied ? undefined : (
          <button className="btn btn-sm" onClick={retry}>
            Retry
          </button>
        )
      }
    >
      {denied
        ? "This information is unavailable to you in this space. Ask a space owner for access."
        : `Could not load ${subject}. Try again.`}
    </ErrorLine>
  );
}
