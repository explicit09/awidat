import { create } from "zustand";

export type UploadAccountOption = {
  id: string;
  provider: string;
  displayName: string;
  handle?: string | null;
  providerAccountId?: string;
  capabilities: { uploadVideo: boolean };
};

type UploadAccountSelectionsState = {
  byProvider: Record<string, string>;
  setSelected: (provider: string, accountId: string) => void;
  clearSelected: (provider: string) => void;
};

const STORAGE_KEY = "montage.deliver.uploadAccountSelections.v1";

function loadLocal(): Record<string, string> {
  if (typeof localStorage === "undefined") return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, string> = {};
    for (const [provider, accountId] of Object.entries(parsed)) {
      if (typeof provider === "string" && typeof accountId === "string") {
        out[provider] = accountId;
      }
    }
    return out;
  } catch {
    return {};
  }
}

function persistLocal(byProvider: Record<string, string>): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(byProvider));
  } catch {
    // localStorage may be unavailable; the in-memory selection still works.
  }
}

export function uploadCapableAccountsForProvider(
  accounts: UploadAccountOption[],
  provider: string,
): UploadAccountOption[] {
  return accounts.filter(
    (account) =>
      account.provider === provider && account.capabilities.uploadVideo,
  );
}

export function selectedAccountIdForProvider(
  accounts: UploadAccountOption[],
  provider: string,
  preferredAccountId: string | undefined,
): string | undefined {
  const uploadCapable = uploadCapableAccountsForProvider(accounts, provider);
  if (preferredAccountId && uploadCapable.some((account) => account.id === preferredAccountId)) {
    return preferredAccountId;
  }
  return uploadCapable[0]?.id;
}

export function accountLabelForProvider(
  accounts: UploadAccountOption[],
  provider: string,
  preferredAccountId: string | undefined,
): string {
  const accountId = selectedAccountIdForProvider(accounts, provider, preferredAccountId);
  const account = accountId ? accounts.find((item) => item.id === accountId) : undefined;
  return (
    account?.displayName ||
    account?.handle ||
    account?.providerAccountId ||
    accountId ||
    "No upload-capable account"
  );
}

export function selectedAccountIdsForProviders(
  accounts: UploadAccountOption[],
  providers: string[],
  preferredByProvider: Record<string, string>,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const provider of providers) {
    const accountId = selectedAccountIdForProvider(
      accounts,
      provider,
      preferredByProvider[provider],
    );
    if (accountId) out[provider] = accountId;
  }
  return out;
}

export function shouldUploadRenderTarget(
  renderTarget: string,
  selectedTargets: ReadonlySet<string>,
  uploadEnabledTargets: ReadonlySet<string>,
): boolean {
  return selectedTargets.has(renderTarget) && uploadEnabledTargets.has(renderTarget);
}

export const useUploadAccountSelections = create<UploadAccountSelectionsState>(
  (set, get) => ({
    byProvider: loadLocal(),
    setSelected: (provider, accountId) => {
      const next = { ...get().byProvider, [provider]: accountId };
      set({ byProvider: next });
      persistLocal(next);
    },
    clearSelected: (provider) => {
      const next = { ...get().byProvider };
      delete next[provider];
      set({ byProvider: next });
      persistLocal(next);
    },
  }),
);
