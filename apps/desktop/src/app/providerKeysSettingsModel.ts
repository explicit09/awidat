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

export function providerKeyStatusLabel(row: ProviderKeyRow): string {
  return row.status === "configured" ? "Configured" : "Not set";
}

export function providerKeyActionLabel(row: ProviderKeyRow): string {
  return row.status === "configured" ? "Replace" : "Add";
}

export function providerKeySubtitle(row: ProviderKeyRow): string {
  return `${row.capability} · ${row.envVar}`;
}
