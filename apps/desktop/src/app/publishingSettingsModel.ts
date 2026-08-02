// Shared provider catalog for server-backed publishing surfaces.

import type { DeliveryTargetKey } from "../shell/delivery/types";
import type { Provider } from "./social/socialModel";

/** The providers the publishing pipeline exposes. */
export const PROVIDERS: ReadonlyArray<{
  key: Provider;
  displayName: string;
}> = [
  {
    key: "youtube",
    displayName: "YouTube",
  },
  {
    key: "tiktok",
    displayName: "TikTok",
  },
  {
    key: "instagram",
    displayName: "Instagram",
  },
  {
    key: "twitter_x",
    displayName: "Twitter/X",
  },
];

export const VISIBLE_PROVIDERS = PROVIDERS.filter(
  (provider) => provider.key !== "tiktok" && provider.key !== "instagram",
);

export function providerDisplayName(key: DeliveryTargetKey): string {
  return PROVIDERS.find((provider) => provider.key === key)?.displayName ?? key;
}
