import { clearPreviewLutCache } from "../media/previewLutCache.ts";
import { clearStripCache } from "../timeline/thumbnailCache.ts";
import { clearWaveformCache } from "../timeline/waveformCache.ts";
import { useTranscriptStore } from "../transcript/store.ts";

export function clearProjectScopedFrontendState(): void {
  const transcript = useTranscriptStore.getState();
  transcript.clearCache();
  transcript.setActiveStem(null);
  clearStripCache();
  clearWaveformCache();
  clearPreviewLutCache();
}
