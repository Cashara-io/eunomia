import { useMutation } from "@tanstack/react-query";
import { changePassword, forgotPassword, resetPassword } from "@/lib/gen";
import { call } from "./client";

/** Asks the server to email a password reset link. It answers the same for any address, so the caller says the same too. */
export const useForgotPassword = () => useMutation({ mutationFn: (email: string) => call(forgotPassword({ body: { email } })) });

/** Sets a new password with the token from a reset link. Signs the account out everywhere; starts no session. */
export const useResetPassword = () =>
  useMutation({ mutationFn: ({ token, password }: { token: string; password: string }) => call(resetPassword({ body: { token, password } })) });

/** Changes the signed-in account's password. Its other sessions end, this one stays. */
export const useChangePassword = () =>
  useMutation({
    mutationFn: (body: { current_password: string; new_password: string; mfa_code?: string }) => call(changePassword({ body })),
  });
