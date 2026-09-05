import {
  Captions,
  FileImage,
  FileVideo,
  Image as ImageIcon,
  MessageCircle,
  Play,
  Square,
  type LucideIcon,
} from "lucide-react";
import { siInstagram, siTiktok, siYoutube } from "simple-icons";
import type { SimpleIconShape } from "../../ui";
import type { DeliveryTargetKey } from "./types";
import type { RenderQueueEntry } from "../../app/renderQueue";

export type TargetMeta = {
  /** Lucide fallback (used by asset rows: captions / cover / custom). */
  icon: LucideIcon;
  /** Official brand glyph for platform rows. When present, used in
   *  place of the lucide icon and tinted with the brand color when
   *  the row is selected. */
  brand?: SimpleIconShape;
  label: string;
  spec: string;
  kind: "video" | "asset";
};

export const TARGET_META: Record<DeliveryTargetKey, TargetMeta> = {
  youtube: {
    icon: Play,
    brand: siYoutube,
    label: "YouTube",
    spec: "1080p · 16:9 · h264",
    kind: "video",
  },
  tiktok: {
    icon: FileVideo,
    brand: siTiktok,
    label: "TikTok",
    spec: "1080p · 9:16 · h264",
    kind: "video",
  },
  instagram: {
    icon: Square,
    brand: siInstagram,
    label: "Instagram",
    spec: "1080p · 1:1 / 9:16",
    kind: "video",
  },
  twitter_x: {
    icon: MessageCircle,
    label: "Twitter/X",
    spec: "1080p · 9:16 · h264",
    kind: "video",
  },
  captions: { icon: Captions, label: "Captions", spec: "SRT + VTT", kind: "asset" },
  cover: { icon: ImageIcon, label: "Cover", spec: "1280×720 PNG", kind: "asset" },
  custom: { icon: FileImage, label: "Custom frame", spec: "User-selected", kind: "asset" },
};

/** Map a render-queue entry's `kind` back to the originating target
 *  key so we can show "this target is rendering" on the left rail. */
export function targetKeyForKind(
  kind: RenderQueueEntry["kind"],
  label: string,
): DeliveryTargetKey | null {
  if (kind === "captions") return "captions";
  if (kind === "still") {
    // Cover/custom both produce stills. The label disambiguates.
    return label.toLowerCase().includes("custom") ? "custom" : "cover";
  }
  if (kind === "video_master") return "youtube";
  if (kind === "video_reframe") {
    const lc = label.toLowerCase();
    if (lc.includes("twitter") || lc.includes("x ")) return "twitter_x";
    if (lc.includes("tiktok") || lc.includes("9:16")) return "tiktok";
    if (lc.includes("instagram") || lc.includes("1:1")) return "instagram";
    return "tiktok";
  }
  return null;
}

/** Targets that support publishing after render. */
export function isUploadCapableTarget(key: DeliveryTargetKey): boolean {
  return key === "youtube" || key === "twitter_x";
}
