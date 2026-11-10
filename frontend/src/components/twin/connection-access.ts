import { ApiError } from "@/lib/api";

// Read routes use vault.forbidden for membership; entity reads can mask it as not-found.
export function spaceDenied(error: unknown, spaceLevel: boolean): error is ApiError {
  return error instanceof ApiError && (
    (error.status === 403 && ["vault.forbidden", "auth.scope", "tenant.denied"].includes(error.code)) ||
    (spaceLevel && error.status === 404)
  );
}

export function objectMissing(error: unknown): error is ApiError {
  return error instanceof ApiError && error.status === 404 &&
    ["entity.not_found", "resource.not_found", "scope.mismatch"].includes(error.code);
}
