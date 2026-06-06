import type { DeliveryTargetKey } from "../app/deliveryTargets";

export type EditorPublishingSummaryInput = {
  selectedTargets: ReadonlySet<DeliveryTargetKey>;
  uploadTargets: ReadonlySet<DeliveryTargetKey>;
  accountSelections: Record<string, string>;
};

export type EditorPublishingSummary = {
  selectedCount: number;
  uploadCount: number;
  accountCount: number;
  readyToExport: boolean;
  copy: string;
};

export function summarizeEditorPublishing(
  input: EditorPublishingSummaryInput,
): EditorPublishingSummary {
  const selectedCount = input.selectedTargets.size;
  const uploadProviders = [...input.uploadTargets].filter((target) =>
    input.selectedTargets.has(target),
  );
  const accountCount = uploadProviders.filter((provider) =>
    Boolean(input.accountSelections[provider]),
  ).length;
  const readyToExport = selectedCount > 0;
  const uploadSuffix = uploadProviders.length === 1 ? "" : "s";
  const accountSuffix = accountCount === 1 ? "" : "s";
  const targetSuffix = selectedCount === 1 ? "" : "s";
  const copy =
    selectedCount === 0
      ? "No delivery targets selected"
      : uploadProviders.length === 0
        ? `${selectedCount} export target${targetSuffix} selected`
        : `${uploadProviders.length} social upload${uploadSuffix} selected · ${accountCount} account${accountSuffix} set`;

  return {
    selectedCount,
    uploadCount: uploadProviders.length,
    accountCount,
    readyToExport,
    copy,
  };
}
