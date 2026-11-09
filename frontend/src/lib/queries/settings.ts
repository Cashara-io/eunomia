import { haptic } from "@/lib/haptics";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { checkForUpdate, completeOnboarding, getHttpsStatus, getSettings, getUpdateStatus, listOpenaiModels, requestHttps, requestUpdate, updateSettings } from "@/lib/gen";
import type { SettingsUpdate } from "@/lib/gen";
import type { AppSettings, HttpsStatus, OpenAiModels, UpdateStatus } from "@/lib/types";
import { call } from "./client";
import { authKeys } from "./auth";

export const settingsKeys = {
  all: ["settings"] as const,
  get: () => [...settingsKeys.all, "get"] as const,
  update: () => [...settingsKeys.all, "update"] as const,
  models: (baseUrl: string, keySet: boolean) => [...settingsKeys.all, "openai-models", baseUrl, keySet] as const,
  updateStatus: () => [...settingsKeys.all, "update-status"] as const,
  httpsStatus: () => [...settingsKeys.all, "https-status"] as const,
};

export const useSettings = (enabled = true) =>
  useQuery({ queryKey: settingsKeys.get(), queryFn: () => call(getSettings()) as Promise<AppSettings>, enabled });

export function useUpdateSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: SettingsUpdate) => call(updateSettings({ body })) as Promise<AppSettings>,
    onSuccess: (s) => {
      haptic("success");
      qc.setQueryData(settingsKeys.get(), s);
      // The model list depends on the saved key and base URL.
      return qc.invalidateQueries({ queryKey: [...settingsKeys.all, "openai-models"] });
    },
  });
}

export function useCompleteOnboarding() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => call(completeOnboarding()),
    onSuccess: () => qc.invalidateQueries({ queryKey: authKeys.me() }),
  });
}

/** Models the configured OpenAI-compatible endpoint offers; refetches when the saved URL or key changes. */
export const useOpenAiModels = (baseUrl: string, keySet: boolean) =>
  useQuery({ queryKey: settingsKeys.models(baseUrl, keySet), queryFn: () => call(listOpenaiModels()) as Promise<OpenAiModels> });

export const updateStatusQuery = () => ({
  queryKey: settingsKeys.updateStatus(),
  queryFn: () => call(getUpdateStatus()) as Promise<UpdateStatus>,
});

export const useUpdateStatus = () => useQuery(updateStatusQuery());

// Both just drop a marker for the updater; the Updates tab polls status for the outcome.
export const useRequestUpdate = () => useMutation({ mutationFn: () => call(requestUpdate()) });
export const useCheckForUpdate = () => useMutation({ mutationFn: () => call(checkForUpdate()) });

/** `refetchInterval` follows the state: quick while a certificate is being fetched, lazy otherwise. */
export const useHttpsStatus = () =>
  useQuery({
    queryKey: settingsKeys.httpsStatus(),
    queryFn: () => call(getHttpsStatus()) as Promise<HttpsStatus>,
    refetchInterval: (q) => (q.state.data?.configured && q.state.data.state === "pending" ? 3000 : 15000),
  });

/** Only drops a request file for the updater; the status query shows the outcome. */
export function useRequestHttps() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { enabled: boolean; domain?: string; email?: string }) => call(requestHttps({ body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: settingsKeys.httpsStatus() }),
  });
}
