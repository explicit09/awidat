type GeneratedMediaCostFields = {
  cost_estimate_usd?: number | null;
  cost_actual_usd?: number | null;
};

export function generatedMediaCostLabel(entry: GeneratedMediaCostFields): string {
  if (typeof entry.cost_actual_usd === "number") {
    return `Actual cost ${formatUsd(entry.cost_actual_usd)}`;
  }
  if (typeof entry.cost_estimate_usd === "number") {
    return `Estimated cost ${formatUsd(entry.cost_estimate_usd)}`;
  }
  return "cost unknown";
}

function formatUsd(value: number): string {
  return `$${value.toFixed(2)}`;
}
