export type ProviderKeyStatus = "notSet" | "configured";

export type ProviderKeyRow = {
  key: string;
  label: string;
  account: string;
  envVar: string;
  capability: string;
  status: ProviderKeyStatus;
  redacted: string | null;
};

export type ProviderKeyTestResult = {
  key: string;
  ok: boolean;
  message: string;
};

export type ProviderKeyImportSummary = {
  imported: string[];
  rows: ProviderKeyRow[];
};

const SETUP_URLS: Readonly<Record<string, string>> = {
  hugging_face: "https://huggingface.co/settings/tokens",
  deepgram: "https://console.deepgram.com/",
  openrouter: "https://openrouter.ai/settings/keys",
  anthropic: "https://console.anthropic.com/settings/keys",
  pexels: "https://www.pexels.com/api/",
  x: "https://developer.x.com/",
};

export function providerKeyStatusLabel(row: ProviderKeyRow): string {
  return row.status === "configured" ? "Configured" : "Not set";
}

export function providerKeyActionLabel(row: ProviderKeyRow): string {
  return row.status === "configured" ? "Replace" : "Add";
}

export function providerKeySubtitle(row: ProviderKeyRow): string {
  return `${row.capability} · ${row.envVar}`;
}

export function providerKeySetupUrl(row: ProviderKeyRow): string | null {
  return SETUP_URLS[row.key] ?? null;
}
