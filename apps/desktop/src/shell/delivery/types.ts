import type { PreflightSeverity } from "../../ui";

export type DeliveryTargetKey =
  | "youtube"
  | "tiktok"
  | "instagram"
  | "twitter_x"
  | "captions"
  | "cover"
  | "custom";

export type DeliveryTarget = {
  key: DeliveryTargetKey;
  /** Selected by the user. */
  active: boolean;
  /** Optional human label override. */
  label?: string;
  /** Optional one-line spec ("1080p · 16:9 · h264"). */
  spec?: string;
};

export type PreflightFinding = {
  id: string;
  severity: PreflightSeverity;
  time?: string;
  message: string;
  asset?: string;
  suggestedFix?: string;
};

export type DeliveryRenderSummary = {
  duration: string;
  estimatedSize?: string;
  outputs: number;
  /** 0..1 — how confident the system is that the render will be clean. */
  confidence: number;
};

export const ALL_TARGETS: DeliveryTargetKey[] = [
  "youtube",
  "twitter_x",
  "captions",
  "cover",
  "custom",
];

export function countBySeverity(findings: PreflightFinding[]) {
  const c: Record<PreflightSeverity, number> = {
    pass: 0,
    info: 0,
    warning: 0,
    error: 0,
    failure: 0,
  };
  for (const f of findings) c[f.severity]++;
  return c;
}
