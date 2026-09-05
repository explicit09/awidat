import type { AgentProfile } from "../protocol";

export const AGENT_PROFILE_LABELS: Record<AgentProfile, string> = {
  balanced: "Balanced",
  deep_edit: "Deep Edit",
};

export const AGENT_PROFILE_OPTIONS: ReadonlyArray<{
  value: AgentProfile;
  label: string;
  description: string;
}> = [
  {
    value: "balanced",
    label: AGENT_PROFILE_LABELS.balanced,
    description: "Routine cleanup and mechanical editing",
  },
  {
    value: "deep_edit",
    label: AGENT_PROFILE_LABELS.deep_edit,
    description: "Story, montage, transitions, and visual review",
  },
];
