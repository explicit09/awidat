/**
 * Awidat v2 App — composes the AppShell from src/shell with real wiring.
 *
 * The legacy 3-column layout has been retired (history: see git on
 * `ui-v2`). Side effects that the legacy App.tsx owned (Tauri channel
 * subscribers, menu-command routing, project lifecycle) live in
 * `src/state/appGlue.ts`. Everything below this point is pure
 * composition + glue between Zustand stores and shell components.
 */

import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { editorDispatch } from "./editor/tauriDispatch";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { PanelRightOpen } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useAgentStore } from "./agent/store";
import { itemsToConversationTurns } from "./agent/conversationTurns";
import { buildTurnContext, chatHistoryLoader } from "./agent/turnContext";
import { useProjectStore } from "./app/state";
import { AgentsMdEditor } from "./app/AgentsMdEditor";
import { NewProjectForm } from "./app/NewProjectForm";
import { SettingsModal } from "./app/SettingsModal";
import { WelcomeCard } from "./app/WelcomeCard";
import { useMediaStore } from "./media/store";
import { GeneratedMediaPanel } from "./media/GeneratedMediaPanel";
import { useGeneratedMediaStore, type GeneratedMediaEntry } from "./media/generatedMediaStore";
import { mediaStreamUrl } from "./media/mediaStreamUrl";
import { resolvePreviewMedia, type PreviewQualityMode } from "./media/previewSource";
import { SegmentedVideoView } from "./media/SegmentedVideoView";
import { MediaOfflineBanner } from "./media/MediaOfflineBanner";
import { findMediaReadinessEntry, mediaReadinessUi } from "./media/readiness";
import { useTranscriptStore } from "./transcript/store";
import { TranscriptView } from "./transcript/TranscriptView";
import { VeditPanel } from "./vedit/VeditPanel";
import {
  AppShell,
  StageShell,
  CommandRail,
  DeliverySurface,
  DRAFT_METADATA_JOB_ID,
  SkillsSurface,
  HistorySurface,
  BriefSurface,
  CenterModeTabs,
  IndexRail,
  isTranscriptFirstProjectType,
  PreviewSurface,
  TranscriptSource,
  ProposalInspector,
  TimelineHybrid,
  type ActivityEntry,
  type ChatSessionSummary,
  type ContextChip,
  type ConversationTurn,
  type MediaSuggestion,
  type DeliveryRenderSummary,
  type DeliveryTarget,
  type IndexingMediaItem,
  type IndexingEpisodeSummary,
  type IndexingStructurePreview,
  type IndexingTask,
  type IndexerConfigEntry,
  type IndexerConfigSnapshot,
  type PlanItem,
  type PreflightFinding,
  type PreviewChange,
  type PreviewViewMode,
  type TimelineTab,
  type TimelineViewMode,
} from "./shell";
import { Footer as ChromeFooter } from "./shell/chrome/Footer";
import { Landing } from "./shell/empty/Landing";
import { Button, Card, Inline, Stack, StatusPillFromMapping, type MediaIndexingStatus, type StatusPillMapping } from "./ui";
import { ClipInspector } from "./inspector/ClipInspector";
import { useStageStore } from "./state";
import { useAppGlue } from "./state/appGlue";
import { useIndexReadinessStore } from "./state/indexReadiness";
import { useEpisodesStore } from "./state/episodes";
import { useIntroState } from "./state/introState";
import { useBriefProposalsStore } from "./state/briefProposals";
import { useCenterModeStore, type CenterMode } from "./state/centerMode";
import { installDefaultAdapter as installFocusAdapter } from "./state/focusController";
import { useSettings } from "./state/settings";
import {
  providerKeyForTarget,
  useUploadPrefs,
} from "./state/uploadPrefs";
import { useUploadMetadata } from "./state/uploadMetadata";
import { useRenderQueueWorker } from "./app/useRenderQueueWorker";
import {
  DELIVERY_TARGETS,
  useDeliveryTargetsStore,
  type DeliveryTargetKey,
} from "./app/deliveryTargets";
import {
  newQueueId,
  useRenderQueueStore,
  type RenderQueueEntry,
} from "./app/renderQueue";
import { useProposalInspectorData } from "./state/proposalAdapter";
import { useTimelineStore } from "./timeline/store";
import { TimelinePane } from "./timeline/TimelinePane";
import { useProposalStore } from "./timeline/proposal";
import { MENU_COMMANDS, onMenuCommand } from "./app/menuCommands";
import type { Item, JobKind, MediaCacheReadiness, MediaReadinessSnapshot, PermissionMode, TimelineSnapshot } from "./protocol";
import { TIMELINE_CHANGED_EVENT } from "./protocol";
import {
  screen2Activity,
  SCREEN2_CURRENT_TIME_S,
  SCREEN2_DURATION_S,
  Screen2MediaSlot,
  screen2AudioPeaks,
  screen2Changes,
  screen2ContextChips,
  screen2Inspector,
  screen2Plan,
  screen2Suggestions,
} from "./shell/screen2Demo";
import { demoScreens, resolveDemoScreenId } from "./shell/demoScreens";
import "./ui/tokens.css";
import "./App.css";
import "./ui/glass.css";
import { AmbientBackground } from "./ui/glass";

function App() {
  // Side effects (Tauri channels, menu routing, project lifecycle).
  useAppGlue();
  // Drive the Deliver-page render queue: drains pending entries one
  // at a time through the appropriate Tauri command. Sits at the
  // root so it survives Deliver-tab unmounts and continues exports
  // when the user switches back to Edit.
  useRenderQueueWorker();
  // Pull the persisted "Upload after render?" opt-ins from the
  // backend once on mount. Local mirror in localStorage means the UI
  // doesn't flash an "off" state in the meantime.
  useEffect(() => {
    void useUploadPrefs.getState().hydrate();
  }, []);

  const current = useProjectStore((s) => s.current);
  const refreshProject = useProjectStore((s) => s.refresh);
  const projectType = useProjectStore((s) => s.projectType);
  const items = useAgentStore((s) => s.items);
  const running = useAgentStore((s) => s.running);
  const replaceAgentItems = useAgentStore((s) => s.replace);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setActiveTurnId = useAgentStore((s) => s.setActiveTurnId);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const stage = useStageStore((s) => s.current);
  const setStage = useStageStore((s) => s.set);
  // 2026 "Stage" shell. Flip to false to fall back to the three-rail cockpit.
  const STAGE_SHELL = true;

  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const timelineSnapshot = useTimelineStore((s) => s.snapshot);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
  const sourceCurrentTimeS = useMediaStore((s) => s.currentTime);
  const sourceDurationS = useMediaStore((s) => s.durationS);
  const timelineTimeS = useMediaStore((s) => s.timelineTime);
  const isPlaying = useMediaStore((s) => s.isPlaying);
  const setSourceTime = useMediaStore((s) => s.setTime);
  const setSourceDuration = useMediaStore((s) => s.setDuration);
  const setMediaPlaying = useMediaStore((s) => s.setPlaying);
  const requestSourceSeek = useMediaStore((s) => s.requestSeek);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const refreshMedia = useMediaStore((s) => s.refresh);
  const selectMedia = useMediaStore((s) => s.select);
  const sourceSeekRequestId = useMediaStore((s) => s.seekRequestId);
  const sourceSeekTargetS = useMediaStore((s) => s.seekTargetS);
  const sources = useMediaStore((s) => s.sources);
  const proxies = useMediaStore((s) => s.proxies);
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const generatedMedia = useGeneratedMediaStore((s) => s.entries);
  const generatedMediaLoading = useGeneratedMediaStore((s) => s.loading);
  const generatedMediaError = useGeneratedMediaStore((s) => s.error);
  const refreshGeneratedMedia = useGeneratedMediaStore((s) => s.refresh);
  const clearGeneratedMedia = useGeneratedMediaStore((s) => s.clear);
  const setActiveTranscriptStem = useTranscriptStore((s) => s.setActiveStem);
  const transcriptState = useTranscriptStore((s) =>
    selectedStem ? s.byStem[selectedStem] : undefined,
  );

  const activeProposal = useProposalStore((s) => s.active);
  const inspectorData = useProposalInspectorData();

  // Wave 3 B1: Brief / Source / Timeline center-pane toggle. The store
  // resolves the active mode per-project with a default rule (Brief
  // when the agent has work waiting or the project hasn't been
  // introduced yet; Source otherwise).
  //
  // Subscribe to the slice so re-renders fire after every set(). The
  // approvals subscription on the brief store keeps `pending().length`
  // current; intro state is read once per project change via hasIntroduced.
  useBriefProposalsStore((s) => s.approvals);
  useCenterModeStore((s) => s.byProject);
  const briefPendingCount = useBriefProposalsStore.getState().pending().length;
  const hasIntroduced = useIntroState((s) => s.hasIntroduced);
  const centerModeStoreGet = useCenterModeStore.getState().get;
  const setCenterModeStore = useCenterModeStore.getState().set;
  const centerMode: CenterMode = centerModeStoreGet(current, {
    pendingCount: briefPendingCount,
    isFirstSession: current ? !hasIntroduced(current) : false,
  });
  const setCenterMode = (next: CenterMode) => setCenterModeStore(current, next);

  // Wave 4 W4.6 — wire the focus controller. The adapter closes over
  // the latest `current` (project root) + setCenterMode at the time of
  // the effect, so a project switch re-installs the adapter and the
  // controller drives the right project's tab. `installFocusAdapter`
  // only swaps the singleton's adapter ref — it never re-creates the
  // controller, so subscribers stay attached across re-installs.
  useEffect(() => {
    installFocusAdapter({
      setCenterMode: (next: CenterMode) => {
        if (current) setCenterModeStore(current, next);
      },
      requestTimelineSeek: (t) => {
        useMediaStore.getState().requestTimelineSeek(t);
      },
      scrollTimelineTo: (centerTimeS) => {
        // The timeline-stage's scrollLeft governs horizontal scroll.
        // We translate seconds → pixels via the canvas's width / the
        // snapshot duration — the canvas only renders once it has both,
        // so when either is zero we skip silently.
        const stage = document.querySelector<HTMLElement>(".timeline-stage");
        const canvas = stage?.querySelector<HTMLCanvasElement>(".timeline-canvas");
        if (!stage || !canvas) return;
        const canvasWidth = canvas.clientWidth;
        const duration = useTimelineStore.getState().snapshot.duration_s;
        if (canvasWidth <= 0 || duration <= 0) return;
        const pps = canvasWidth / duration;
        const targetX = centerTimeS * pps;
        const viewportWidth = stage.clientWidth;
        const desiredLeft = Math.max(0, targetX - viewportWidth / 2);
        stage.scrollTo({ left: desiredLeft, behavior: "smooth" });
      },
      readTimelineSnapshot: () => useTimelineStore.getState().snapshot,
    });
  }, [current, setCenterModeStore]);

  const [timelineTab, setTimelineTab] = useState<TimelineTab>("timeline");
  const [timelineViewMode, setTimelineViewMode] = useState<TimelineViewMode>("proposed");
  const [previewViewMode, setPreviewViewMode] = useState<PreviewViewMode>("before-after");
  const [previewQualityMode, setPreviewQualityMode] = useState<PreviewQualityMode>("auto");
  const [previewVolume, setPreviewVolume] = useState(0.8);
  const [previewRate, setPreviewRate] = useState(1);
  const [activePreviewChangeId, setActivePreviewChangeId] = useState<string | undefined>(undefined);
  const [, setRealVideoFrames] = useState<string[]>([]);
  const [realAudioPeaks, setRealAudioPeaks] = useState<number[]>([]);
  const [realPreviewSrc, setRealPreviewSrc] = useState<string | null>(null);
  // Tracks whether the current `RealMediaPreviewSlot` has decoded its
  // first frame. Lifted out of the slot so `PreviewSurface` can render
  // the `FilmSlate` overlay until the swap and cross-fade it out once
  // the player has something paintable. Reset to false whenever the
  // selected media changes (see effect below) so re-selecting a clip
  // re-shows the slate while its frame loads.
  const [hasProxyFrame, setHasProxyFrame] = useState(false);
  const mediaReadinessCommandUnavailableRef = useRef(false);
  const [showNewProject, setShowNewProject] = useState(false);
  const [pendingImportPaths, setPendingImportPaths] = useState<string[] | null>(null);
  const [showUrlImport, setShowUrlImport] = useState(false);
  const [pendingImportUrl, setPendingImportUrl] = useState<string | null>(null);
  const [urlInput, setUrlInput] = useState("");
  const [commandError, setCommandError] = useState<string | null>(null);
  const [dismissedContextChips, setDismissedContextChips] = useState<string[]>([]);
  const [indexerConfig, setIndexerConfig] = useState<IndexerConfigSnapshot | undefined>(undefined);
  const [indexReadiness, setIndexReadiness] = useState<IndexReadinessSnapshot | undefined>(undefined);
  const [episodeSummary, setEpisodeSummary] = useState<IndexingEpisodeSummary | undefined>(undefined);
  const [mediaReadiness, setMediaReadiness] = useState<MediaReadinessSnapshot | undefined>(undefined);
  const [runningJobIds, setRunningJobIds] = useState<Set<string> | undefined>(undefined);
  const [chatSessions, setChatSessions] = useState<ChatSessionSummary[]>([]);
  const [activeChatSession, setActiveChatSession] = useState<ChatSessionSummary | null>(null);
  const [chatLoading, setChatLoading] = useState(false);
  const [agentFocusMode, setAgentFocusMode] = useState(false);
  const [permissionMode, setPermissionModeState] = useState<PermissionMode>("manual");
  // Default-expanded: the saved layout in localStorage gives the
  // Inspector a real proportion (~21%) and the user expects that
  // width back on reload. Starting collapsed loaded a different
  // layout variant (`awidat.shell.h.inspector-collapsed`) and gave
  // the Inspector a 2% stub regardless of what was saved. The auto-
  // expand-on-proposal effect below still kicks in when a proposal
  // arrives, but boot doesn't force the collapse anymore.
  const [inspectorCollapsed, setInspectorCollapsed] = useState(false);
  const [leftPanel, setLeftPanel] = useState<"agent" | "media">("agent");
  const [rightPanel, setRightPanel] = useState<"inspector" | "index" | "transcript" | "vedit">("inspector");
  // The bottom dock used to host Timeline / Transcript / Vedit as
  // sibling tabs with docked / collapsed / popout chrome. Transcript +
  // Vedit moved to the right rail; the dock is now the timeline only,
  // so the tab + dock-state hooks went with them.

  const hasProject = current !== null;
  const demoMode = !hasProject && !isTauri();
  const demoScreenId = demoMode
    ? typeof window !== "undefined" && window.location.pathname === "/design/concept"
      ? "screen1"
      : resolveDemoScreenId(typeof window === "undefined" ? "" : window.location.search)
    : "screen2";
  const demoScreen = demoScreens[demoScreenId];
  const selectedProxy = useMemo(
    () =>
      proxies.find(
        (proxy) =>
          proxy.stem === selectedStem ||
          (selectedStem !== null && proxy.stem.startsWith(`${selectedStem}-`)),
      ) ??
      proxies[0] ??
      null,
    [proxies, selectedStem],
  );
  const selectedSource = useMemo(() => {
    if (sources.length === 0) return null;
    if (!selectedStem) return sources[0] ?? null;
    return (
      sources.find((source) => source.name.startsWith(selectedStem)) ??
      sources[0] ??
      null
    );
  }, [selectedStem, sources]);
  const selectedPreviewMedia = useMemo(() => {
    return resolvePreviewMedia({
      mode: previewQualityMode,
      source: selectedSource,
      proxy: selectedProxy,
    });
  }, [previewQualityMode, selectedProxy, selectedSource]);
  const sourceMediaCount = Math.max(sources.length, proxies.length);
  const hasImportedMedia = sourceMediaCount > 0;
  const routedProjectRef = useRef<{
    project: string | null;
    mode: "empty" | "proxy" | "timeline" | "proposal" | null;
  }>({ project: null, mode: null });

  async function importFiles(paths: string[]) {
    if (paths.length === 0) return;
    setCommandError(null);
    try {
      const imported = await invoke<string[]>("import_locals", { srcPaths: paths, link: false });
      await refreshMedia();
      selectMedia(stemFromPath(imported[imported.length - 1] ?? paths[paths.length - 1]));
      setStage("edit");
      setLeftPanel("media");
      setRightPanel("index");
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function importUrl(url: string) {
    const trimmed = url.trim();
    if (!trimmed) return;
    setCommandError(null);
    try {
      const imported = await invoke<string>("import_url", { url: trimmed });
      await refreshMedia();
      selectMedia(stemFromPath(imported));
      setStage("edit");
      setLeftPanel("media");
      setRightPanel("index");
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function placeMediaOnTimeline(assetId: string, atS?: number) {
    if (!isTauri()) return;
    setCommandError(null);
    try {
      await invoke<boolean>("insert_media_on_timeline", {
        assetId,
        atS: atS ?? null,
      });
      await refreshTimeline();
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function placeGeneratedMediaOnTimeline(entry: GeneratedMediaEntry) {
    if (!entry.video_path) return;
    await placeMediaOnTimeline(entry.video_path);
  }

  async function chooseAndImportFiles() {
    if (!isTauri()) return;
    setCommandError(null);
    try {
      const picked = await openDialog({
        directory: false,
        multiple: true,
        title: current ? "Choose media files to import" : "Choose media files for a new project",
      });
      const paths =
        typeof picked === "string"
          ? [picked]
          : Array.isArray(picked)
            ? picked.filter((path): path is string => typeof path === "string")
            : [];
      if (paths.length === 0) return;
      if (current) {
        await importFiles(paths);
      } else {
        setPendingImportPaths(paths);
        setShowNewProject(true);
      }
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function chooseAndOpenProject() {
    if (!isTauri()) return;
    setCommandError(null);
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Open Awidat project",
      });
      if (typeof picked !== "string") return;
      await invoke("set_project_root", { path: picked });
      await refreshProject();
      setStage("edit");
      setLeftPanel("media");
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function completeNewProject(path: string) {
    await refreshProject().catch(() => {});
    setShowNewProject(false);
    setStage("edit");
    const importPaths = pendingImportPaths;
    const importUrlValue = pendingImportUrl;
    setPendingImportPaths(null);
    setPendingImportUrl(null);
    if (importPaths?.length) {
      await importFiles(importPaths);
    } else if (importUrlValue) {
      await importUrl(importUrlValue);
    } else {
      void path;
    }
  }

  function resetSurfaceControls() {
    setDismissedContextChips([]);
    useDeliveryTargetsStore.getState().clear();
  }

  function dismissContextChip(chip: ContextChip) {
    const key = `${chip.kind ?? "tag"}:${chip.label}`;
    setDismissedContextChips((previous) =>
      previous.includes(key) ? previous : [...previous, key],
    );
    // Also drop any matching @-attached clip so it doesn't immediately
    // come back the next time effectiveContextChips is recomputed.
    setPickedMediaChips((previous) => previous.filter((c) => c.label !== chip.label));
  }

  async function runEngineCommand(command: string) {
    const input = command.trim();
    if (!isTauri() || !input) return;
    setCommandError(null);
    setTurnError(null);
    setRunning(true);
    try {
      const turnId = await invoke<string>("start_turn", {
        input,
        context: buildTurnContext(effectiveContextChips),
      });
      setActiveTurnId(turnId);
    } catch (e) {
      if (String(e).includes("turn is already running")) {
        try {
          await invoke("cancel_turn");
          const turnId = await invoke<string>("start_turn", {
            input,
            context: buildTurnContext(effectiveContextChips),
          });
          setActiveTurnId(turnId);
          return;
        } catch (retryErr) {
          setTurnError(String(retryErr));
          setCommandError(String(retryErr));
          setRunning(false);
          return;
        }
      }
      setTurnError(String(e));
      setCommandError(String(e));
      setRunning(false);
    }
  }

  async function runIndexers() {
    if (!isTauri()) return;
    setCommandError(null);
    try {
      await invoke("index_project");
      await Promise.all([
        refreshMedia(),
        useTimelineStore.getState().refresh(),
        loadIndexReadiness(),
        loadProjectEpisodes(),
        loadMediaReadiness(),
      ]);
    } catch (e) {
      setCommandError(String(e));
      await loadIndexReadiness();
      await loadProjectEpisodes();
      await loadMediaReadiness();
    }
  }

  function runTimelineExport() {
    if (!isTauri()) return;
    setCommandError(null);
    // Translate the user's selected delivery targets into render
    // queue entries, ordered so any video_master lands before its
    // video_reframe siblings (the worker hands the master mp4's
    // path to the reframe step).
    const selected = useDeliveryTargetsStore.getState().selected;
    if (selected.size === 0) {
      setCommandError(
        "Pick at least one delivery target before exporting.",
      );
      return;
    }
    const ordered: DeliveryTargetKey[] = [];
    const queueIncludesVideo = Array.from(selected).some(
      (key) =>
        DELIVERY_TARGETS[key].kind === "video_master" ||
        DELIVERY_TARGETS[key].kind === "video_reframe",
    );
    // If any video targets are selected, force the master render to
    // run first so the reframes can consume it. Users who picked
    // only TikTok/Instagram get YouTube enqueued implicitly as the
    // master.
    if (queueIncludesVideo && !selected.has("youtube")) {
      ordered.push("youtube");
    }
    for (const key of selected) {
      if (key !== "youtube" && DELIVERY_TARGETS[key].kind === "video_reframe") {
        // hold reframes until after the master
        continue;
      }
      if (!ordered.includes(key)) ordered.push(key);
    }
    for (const key of selected) {
      if (DELIVERY_TARGETS[key].kind === "video_reframe" && !ordered.includes(key)) {
        ordered.push(key);
      }
    }
    // Per-target auto-upload opt-ins (W5.A2). The target's own key
    // is the provider key (`youtube` → YouTube), so we just gate on
    // whether the user toggled "Upload after render" for that target.
    const uploadEnabled = useUploadPrefs.getState().enabled;
    // Per-target metadata the user drafted in the form (W5.A3). The
    // form persists under DRAFT_METADATA_JOB_ID; on enqueue we copy
    // the per-provider snapshots onto each entry's own id so the
    // worker can hand them to the backend on render-done.
    const metadataStore = useUploadMetadata.getState();
    const entries: RenderQueueEntry[] = ordered.map((key) => {
      const spec = DELIVERY_TARGETS[key];
      const provider = providerKeyForTarget(key);
      const uploadTargets =
        provider && uploadEnabled.has(key) ? [provider] : undefined;
      const entryId = newQueueId(spec.kind);
      // Forward the user's draft metadata for this provider, if any,
      // onto the freshly-allocated entry id so the worker's
      // `useUploadMetadata.get(entry.id, provider)` lookup finds the
      // saved values. Done synchronously through the store action.
      if (uploadTargets && provider) {
        const draft = metadataStore.get(DRAFT_METADATA_JOB_ID, provider, spec.label);
        metadataStore.set(entryId, provider, draft);
      }
      return {
        id: entryId,
        targetId: key,
        label: spec.label,
        kind: spec.kind,
        status: "pending" as const,
        enqueuedAt: Date.now(),
        reframeWidth: spec.width,
        reframeHeight: spec.height,
        reframeBitrateKbps: spec.videoBitrateKbps,
        stillKind: spec.stillKind,
        uploadTargets,
      };
    });
    useRenderQueueStore.getState().enqueue(entries);
  }

  function toggleDeliveryTarget(key: string) {
    if (key in DELIVERY_TARGETS) {
      useDeliveryTargetsStore
        .getState()
        .toggle(key as DeliveryTargetKey);
    }
  }

  function setDeliveryRepair() {
    void runEngineCommand(
      "Please address the current preflight findings and rerun delivery checks.",
    );
  }

  function saveDeliveryPreset() {
    void runEngineCommand("Create a reusable delivery preset from the current settings.");
  }

  function generateVariants() {
    void runEngineCommand(
      "Generate platform variants for all selected delivery targets and summarize any changes.",
    );
  }

  async function loadIndexerConfig() {
    if (demoMode || !isTauri()) {
      setIndexerConfig(undefined);
      return;
    }
    try {
      const snapshot = await invoke<IndexerConfigSnapshot>("read_indexer_config");
      setIndexerConfig(snapshot);
    } catch (e) {
      console.warn("read_indexer_config failed", e);
      setIndexerConfig(undefined);
    }
  }

  async function loadIndexReadiness() {
    if (demoMode || !isTauri() || !current) {
      setIndexReadiness(undefined);
      useIndexReadinessStore.getState().setSnapshot(undefined);
      return;
    }
    try {
      const snapshot = await invoke<IndexReadinessSnapshot>("index_readiness");
      setIndexReadiness(snapshot);
      // Mirror into the shared store so evidenceAccessors and any other
      // surface that doesn't have App-level state access can subscribe.
      // The local React state above stays as-is to avoid a wider refactor
      // of the cockpit slate / footer.
      useIndexReadinessStore.getState().setSnapshot(snapshot);
    } catch (e) {
      console.warn("index_readiness failed", e);
      setIndexReadiness(undefined);
      useIndexReadinessStore.getState().setSnapshot(undefined);
    }
  }

  async function loadProjectEpisodes() {
    if (demoMode || !isTauri() || !current) {
      setEpisodeSummary(undefined);
      // Mirror the local clear into the shared store so accessors that
      // subscribe (EpisodesDrillDown, EvidenceChipRow) drop their data
      // when the project unloads.
      useEpisodesStore.getState().setSnapshot(undefined);
      return;
    }
    try {
      const snapshot = await invoke<ProjectEpisodesResponse>("get_project_episodes");
      const summary = projectEpisodesToIndexingSummary(snapshot);
      setEpisodeSummary(summary);
      useEpisodesStore.getState().setSnapshot(summary);
    } catch (e) {
      console.warn("get_project_episodes failed", e);
      setEpisodeSummary(undefined);
      useEpisodesStore.getState().setSnapshot(undefined);
    }
  }

  async function loadMediaReadiness() {
    if (demoMode || !isTauri() || !current) {
      setMediaReadiness(undefined);
      return;
    }
    if (mediaReadinessCommandUnavailableRef.current) {
      setMediaReadiness(undefined);
      return;
    }
    try {
      const snapshot = await invoke<MediaReadinessSnapshot>("read_media_readiness");
      setMediaReadiness(snapshot);
    } catch (e) {
      const message = String(e);
      if (message.includes("read_media_readiness") && message.includes("not found")) {
        mediaReadinessCommandUnavailableRef.current = true;
      }
      // Backend media-service work may land after the UI. Keep the
      // existing source/proxy-derived labels as the compatibility path.
      setMediaReadiness(undefined);
    }
  }

  async function loadRunningJobIds() {
    if (demoMode || !isTauri() || !current) {
      setRunningJobIds(undefined);
      return;
    }
    try {
      const ids = await invoke<string[]>("running_job_ids");
      setRunningJobIds(new Set(ids));
    } catch (e) {
      console.warn("running_job_ids failed", e);
      setRunningJobIds(new Set());
    }
  }

  async function loadInitialChatHistory() {
    if (demoMode || !isTauri() || !current) {
      setChatSessions([]);
      setActiveChatSession(null);
      return;
    }
    setChatLoading(true);
    try {
      const [sessions, history] = await Promise.all([
        invoke<ChatSessionSummary[]>("list_chat_sessions"),
        invoke<ChatHistory>("load_chat_history"),
      ]);
      setChatSessions(sessions);
      setActiveChatSession(history.session);
      replaceAgentItems(history.items);
    } catch (e) {
      console.warn("chat history load failed", e);
    } finally {
      setChatLoading(false);
    }
  }

  async function refreshChatSessions() {
    if (demoMode || !isTauri() || !current) return;
    try {
      const sessions = await invoke<ChatSessionSummary[]>("list_chat_sessions");
      setChatSessions(sessions);
      setActiveChatSession((active) => {
        if (active) {
          return sessions.find((session) => session.logPath === active.logPath) ?? active;
        }
        return sessions[0] ?? null;
      });
    } catch (e) {
      console.warn("list_chat_sessions failed", e);
    }
  }

  async function selectChatSession(session: ChatSessionSummary) {
    if (!isTauri()) return;
    setChatLoading(true);
    try {
      const history = await chatHistoryLoader<Item>(
        (command, args) => invoke<ChatHistory>(command, args),
        session,
      );
      setActiveChatSession(history.session);
      replaceAgentItems(history.items);
    } catch (e) {
      setCommandError(String(e));
    } finally {
      setChatLoading(false);
    }
  }

  async function startNewChat() {
    if (!isTauri()) return;
    setChatLoading(true);
    try {
      const history = await invoke<ChatHistory>("start_new_chat_session");
      setActiveChatSession(history.session);
      replaceAgentItems(history.items);
    } catch (e) {
      setCommandError(String(e));
    } finally {
      setChatLoading(false);
    }
  }

  async function renameChat(session: ChatSessionSummary, newTitle: string) {
    if (!isTauri()) return;
    try {
      await invoke("rename_chat_session", {
        logPath: session.logPath,
        newTitle,
      });
      await refreshChatSessions();
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function deleteChat(session: ChatSessionSummary) {
    if (!isTauri()) return;
    try {
      await invoke("delete_chat_session", {
        logPath: session.logPath,
      });
      // If the deleted chat was active, drop it from local state too.
      if (activeChatSession?.logPath === session.logPath) {
        setActiveChatSession(null);
        replaceAgentItems([]);
      }
      await refreshChatSessions();
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function toggleProjectIndexer(indexer: IndexerConfigEntry) {
    if (!isTauri()) return;
    setCommandError(null);
    try {
      const snapshot = await invoke<IndexerConfigSnapshot>("set_project_indexer_enabled", {
        args: { name: indexer.name, enabled: !indexer.enabled },
      });
      setIndexerConfig(snapshot);
    } catch (e) {
      setCommandError(String(e));
    }
  }

  function openConfigPath(path: string) {
    if (!isTauri()) return;
    openPath(path).catch((e) => setCommandError(String(e)));
  }

  function revealConfigPath(path: string) {
    if (!isTauri()) return;
    revealItemInDir(path).catch((e) => setCommandError(String(e)));
  }

  function submitUrlImport() {
    const trimmed = urlInput.trim();
    if (!trimmed) return;
    setShowUrlImport(false);
    setUrlInput("");
    if (current) {
      void importUrl(trimmed);
    } else {
      setPendingImportUrl(trimmed);
      setShowNewProject(true);
    }
  }

  function acceptActiveProposal() {
    if (!isTauri() || !activeProposal) return;
    editorDispatch.acceptProposal(activeProposal.callId).catch((e) =>
      console.warn("accept_proposal failed", e),
    );
  }

  function rejectActiveProposal() {
    if (!isTauri() || !activeProposal) return;
    editorDispatch.rejectProposal(activeProposal.callId).catch((e) =>
      console.warn("reject_proposal failed", e),
    );
  }

  // Auto-expand the inspector rail when a proposal lands. Trust-audit
  // is load-bearing for an agent-driven editor — the user shouldn't
  // have to click anything to see *why* the agent suggested a cut.
  // If they manually collapsed it again, that decision survives until
  // the next proposal swap.
  const lastProposalIdRef = useRef<string | null>(null);
  useEffect(() => {
    const currentId = activeProposal?.callId ?? null;
    if (currentId && currentId !== lastProposalIdRef.current) {
      setInspectorCollapsed(false);
      setRightPanel("inspector");
    }
    lastProposalIdRef.current = currentId;
  }, [activeProposal]);

  function inspectActiveProposal() {
    setInspectorCollapsed(false);
    void runEngineCommand("Inspect the selected proposal in detail and list the supporting evidence.");
  }

  function reviseActiveProposal() {
    setInspectorCollapsed(false);
    void runEngineCommand("Revise the selected proposal and explain the tradeoffs.");
  }

  // Chrome handlers `openDeliveryFromChrome` / `openSettingsFromChrome`
  // were removed alongside the old chrome JSX. The new `<TopChrome />`
  // (IdentityRow + WorkspaceRow) is wired in Tasks 8–9; we will reintroduce
  // these once the redesigned chrome handlers land.

  useEffect(() => {
    if (!isTauri()) return;
    void refreshProject();
  }, [refreshProject]);

  useEffect(() => {
    return onMenuCommand((id) => {
      if (id === MENU_COMMANDS.IMPORT_FILES) {
        void chooseAndImportFiles();
      } else if (id === MENU_COMMANDS.IMPORT_URL) {
        setShowUrlImport(true);
      } else if (id === MENU_COMMANDS.OPEN_PROJECT) {
        void chooseAndOpenProject();
      } else if (id === MENU_COMMANDS.NEW_PROJECT) {
        setPendingImportPaths(null);
        setPendingImportUrl(null);
        setShowNewProject(true);
      } else if (id === MENU_COMMANDS.NAV_SETTINGS) {
        useSettings.getState().open();
      }
    });
  });

  // Global keyboard shortcut: ⌘, (or Ctrl+, on non-mac) opens
  // Settings. Mounted once at the App root so the modal can be
  // opened from anywhere. The settings modal handles its own Esc.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const isCommaShortcut = event.key === "," && (event.metaKey || event.ctrlKey);
      if (isCommaShortcut) {
        event.preventDefault();
        useSettings.getState().open();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    if (demoMode) {
      if (stage !== demoScreen.stage) {
        setStage(demoScreen.stage);
      }
    }
  }, [demoMode, demoScreen.stage, setStage, stage]);

  useEffect(() => {
    // Dev-only: when a stage is pinned via VITE_AWIDAT_STAGE (native
    // screenshot tours), don't auto-route the stage back to "edit".
    if (import.meta.env?.VITE_AWIDAT_STAGE) return;
    if (demoMode) {
      routedProjectRef.current = { project: null, mode: null };
      return;
    }
    if (current === null) {
      routedProjectRef.current = { project: null, mode: null };
      setStage("edit");
      return;
    }

    const mode =
      activeProposal !== null
        ? "proposal"
        : timelineDuration > 0
          ? "timeline"
          : hasImportedMedia
            ? "proxy"
            : "empty";

    if (
      routedProjectRef.current.project === current &&
      routedProjectRef.current.mode === mode
    ) {
      return;
    }

    routedProjectRef.current = { project: current, mode };

    if (mode === "proposal" || mode === "timeline") {
      setStage("edit");
      return;
    }

    setStage("edit");
    setRightPanel("index");
  }, [activeProposal, current, demoMode, hasImportedMedia, setStage, timelineDuration]);

  useEffect(() => {
    if (!demoMode) {
      void refreshMedia();
    }
  }, [current, demoMode, refreshMedia]);

  useEffect(() => {
    if (demoMode || !isTauri() || !current) {
      clearGeneratedMedia();
      return;
    }
    void refreshGeneratedMedia();
  }, [clearGeneratedMedia, current, demoMode, refreshGeneratedMedia]);

  useEffect(() => {
    void loadInitialChatHistory();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, demoMode]);

  useEffect(() => {
    if (!running) {
      void refreshChatSessions();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  useEffect(() => {
    resetSurfaceControls();
  }, [current]);

  useEffect(() => {
    if (!demoMode) {
      setActiveTranscriptStem(selectedStem);
    }
  }, [demoMode, selectedStem, setActiveTranscriptStem]);

  useEffect(() => {
    if (demoMode || !isTauri()) {
      setRealVideoFrames([]);
      setRealAudioPeaks([]);
      return;
    }
    const { thumbnailDir, waveformPath } = firstTimelineSidecars(timelineSnapshot);
    let cancelled = false;

    if (thumbnailDir) {
      invoke<string[]>("list_thumbnail_frames", { dir: thumbnailDir })
        .then((paths) => {
          if (cancelled) return;
          setRealVideoFrames(sampleEvenly(paths, 24).map((path) => convertFileSrc(path)));
        })
        .catch((e) => {
          console.warn("list_thumbnail_frames failed", e);
          if (!cancelled) setRealVideoFrames([]);
        });
    } else {
      setRealVideoFrames([]);
    }

    if (waveformPath) {
      invoke<number[]>("read_waveform", { path: waveformPath })
        .then((buckets) => {
          if (cancelled) return;
          setRealAudioPeaks(downsamplePeaks(buckets, 180));
        })
        .catch((e) => {
          console.warn("read_waveform failed", e);
          if (!cancelled) setRealAudioPeaks([]);
        });
    } else {
      setRealAudioPeaks([]);
    }

    return () => {
      cancelled = true;
    };
  }, [demoMode, timelineSnapshot]);

  // Reset the "first frame painted" flag whenever the selected media
  // (or its quality-mode source) changes. Without this, switching to a
  // new clip would skip showing the FilmSlate because the flag would
  // still be true from the previously-loaded media.
  useEffect(() => {
    setHasProxyFrame(false);
  }, [selectedPreviewMedia?.path]);

  useEffect(() => {
    if (demoMode || !isTauri() || !selectedPreviewMedia) {
      setRealPreviewSrc(null);
      return;
    }
    let cancelled = false;
    mediaStreamUrl(selectedPreviewMedia.path)
      .then((url) => {
        if (!cancelled) setRealPreviewSrc(url);
      })
      .catch((e) => {
        console.warn("media preview url failed", e);
        if (!cancelled) setRealPreviewSrc(null);
      });
    return () => {
      cancelled = true;
    };
  }, [demoMode, selectedPreviewMedia]);

  useEffect(() => {
    if (demoMode || !isTauri()) {
      setIndexerConfig(undefined);
      return;
    }
    let cancelled = false;
    invoke<IndexerConfigSnapshot>("read_indexer_config")
      .then((snapshot) => {
        if (!cancelled) setIndexerConfig(snapshot);
      })
      .catch((e) => {
        console.warn("read_indexer_config failed", e);
        if (!cancelled) setIndexerConfig(undefined);
      });
    return () => {
      cancelled = true;
    };
  }, [current, demoMode]);

  // Load persisted agent permission mode so the composer chip shows
  // the right initial value. Falls back to "manual" on error.
  useEffect(() => {
    if (!isTauri()) return;
    invoke<PermissionMode>("get_permission_mode")
      .then((mode) => setPermissionModeState(mode))
      .catch(() => setPermissionModeState("manual"));
  }, []);

  async function changePermissionMode(next: PermissionMode) {
    setPermissionModeState(next);
    if (!isTauri()) return;
    try {
      await invoke("set_permission_mode", { mode: next });
    } catch (e) {
      console.warn("set_permission_mode failed", e);
    }
  }

  // Distill agent items into the CommandRail's plan + activity lists.
  // Real agent producers populate `Plan` items, `ToolCall` items, etc.
  // We translate them into shell-friendly shapes here.
  const plan: PlanItem[] = useMemo(() => {
    const planItems = items.filter(
      (it): it is Extract<typeof items[number], { kind: "plan" }> => it.kind === "plan",
    );
    if (planItems.length === 0) return [];
    // Use the latest Plan item — the agent emits incremental updates.
    const latest = planItems[planItems.length - 1];
    return latest.items.map((step, i) => ({
      id: `${latest.id.toString()}-${i}`,
      text: step.step,
      status: stepStatus(step.status),
    }));
  }, [items]);

  // Activity log now shows *background jobs only* (transcode,
  // thumbnails, waveforms, indexing). Tool calls render inline under
  // their turn — see `turns` below.
  const activity: ActivityEntry[] = useMemo(() => {
    return items
      .filter(
        (it): it is ActivitySourceItem => it.kind === "job",
      )
      .slice(-12)
      .reverse()
      .map((it) => activityFor(it));
  }, [items]);

  // Group items into turns (one per user_input). Inside a turn, the
  // agent's outputs are kept in order: text blocks and tool_calls
  // interleaved as they actually fired. The CommandRail renders the
  // user bubble + the per-turn inline tool/text stream.
  // Use the extracted helper (origin's refactor). It owns sentinel
  // filtering (W3 F3 intro / B4 prepare) and approval-request rendering;
  // see conversationTurns.ts.
  const turns: ConversationTurn[] = useMemo(
    () => itemsToConversationTurns(items, summarizeToolForRail),
    [items],
  );

  const activeJobs = useMemo(
    () =>
      items.filter(
        (it): it is Extract<typeof items[number], { kind: "job" }> =>
          it.kind === "job" &&
          it.phase !== "completed" &&
          runningJobIds !== undefined &&
          runningJobIds.has(it.id),
      ),
    [items, runningJobIds],
  );

  const completedJobKinds = useMemo(() => {
    return new Set(
      items
        .filter(
          (it): it is Extract<typeof items[number], { kind: "job" }> =>
            it.kind === "job" && it.phase === "completed",
        )
        .map((job) => job.job_kind),
    );
  }, [items]);

  useEffect(() => {
    void loadIndexReadiness();
    void loadProjectEpisodes();
    void loadRunningJobIds();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, demoMode, completedJobKinds.size, activeJobs.length, timelineSnapshot.cut_boundaries.length]);

  useEffect(() => {
    mediaReadinessCommandUnavailableRef.current = false;
    void loadMediaReadiness();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, demoMode, completedJobKinds.size, activeJobs.length, sources.length, proxies.length]);

  useEffect(() => {
    if (demoMode || !isTauri() || !current || sourceMediaCount === 0) return;
    const id = window.setInterval(() => {
      void loadMediaReadiness();
      void loadRunningJobIds();
    }, activeJobs.length > 0 ? 2_000 : 5_000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeJobs.length, current, demoMode, sourceMediaCount]);

  // Re-poll readiness when the user comes back to the window. Covers
  // the case where indexer subprocesses kept writing sidecars while
  // the user was in another app, or where the dispatcher's live event
  // stream was lost (orphaned subprocesses from a previous binary
  // session — the disk state is the source of truth either way).
  useEffect(() => {
    function onFocus() {
      void loadIndexReadiness();
      void loadProjectEpisodes();
      void loadMediaReadiness();
      void loadRunningJobIds();
    }
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, demoMode]);

  // Episode metadata lives in OTIO metadata.awidat.episodes, not in
  // timelineSnapshot.cut_boundaries, so the cut-boundary-keyed effect
  // above misses changes from apply_episode_spans. The bridge already
  // emits timeline-changed after the mutating tool completes — re-pull
  // episodes whenever it does for our current project.
  useEffect(() => {
    if (!isTauri() || !current) return;
    const unlisten = listen<string>(TIMELINE_CHANGED_EVENT, (event) => {
      if (event.payload === current) {
        void loadProjectEpisodes();
      }
    });
    return () => {
      unlisten.then((u) => u());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current]);

  // User-attached clips from the composer's @-mention picker. Keyed
  // by the asset id so the same clip can't get attached twice.
  const [pickedMediaChips, setPickedMediaChips] = useState<ContextChip[]>([]);

  // Build the @-mention suggestion pool from the media store. Prefer
  // sources (the canonical asset entries) and fall back to proxies
  // for stems that don't have a matching source — covers imports that
  // haven't finished the post-transcode metadata refresh yet.
  const mediaSuggestions: MediaSuggestion[] = useMemo(() => {
    const stripExt = (name: string) => {
      const i = name.lastIndexOf(".");
      return i > 0 ? name.slice(0, i) : name;
    };
    const seen = new Set<string>();
    const out: MediaSuggestion[] = [];
    for (const src of sources) {
      const stem = stripExt(src.name);
      const short = stem.length > 32 ? `…${stem.slice(-30)}` : stem;
      out.push({
        id: src.id,
        label: short,
        detail: formatBytes(src.size_bytes),
        token: stem,
        chipLabel: `Clip: ${stem}`,
      });
      seen.add(stem);
    }
    for (const proxy of proxies) {
      if (seen.has(proxy.stem)) continue;
      const short = proxy.stem.length > 32 ? `…${proxy.stem.slice(-30)}` : proxy.stem;
      out.push({
        id: proxy.proxy_path,
        label: short,
        detail: formatBytes(proxy.size_bytes),
        token: proxy.stem,
        chipLabel: `Clip: ${proxy.stem}`,
      });
    }
    return out;
  }, [sources, proxies]);

  function attachMediaPick(suggestion: MediaSuggestion) {
    setPickedMediaChips((prev) => {
      if (prev.some((c) => c.label === suggestion.chipLabel)) return prev;
      return [...prev, { label: suggestion.chipLabel, kind: "media" as const }];
    });
  }

  const realContextChips: ContextChip[] = useMemo(() => {
    const chips: ContextChip[] = [];
    if (current) {
      chips.push({ label: `Project: ${projectName(current)}`, kind: "project" });
    }
    if (selectedStem) {
      chips.push({ label: `Clip: ${selectedStem}`, kind: "media" });
    } else if (sourceMediaCount > 0) {
      chips.push({ label: `${sourceMediaCount} source assets`, kind: "media" });
    }
    if (timelineDuration > 0) {
      chips.push({ label: `Timeline: ${formatDuration(timelineDuration)}`, kind: "selection" });
    }
    if (timelineSnapshot.cut_boundaries.length > 0) {
      chips.push({ label: `${timelineSnapshot.cut_boundaries.length} cut boundaries`, kind: "selection" });
    }
    if (activeProposal) {
      chips.push({ label: `Proposal: ${activeProposal.summary}`, kind: "lens" });
    }
    // User-picked @ mentions land at the end so they read as the most
    // recent attachments. De-dup against existing chips first.
    for (const picked of pickedMediaChips) {
      if (!chips.some((c) => c.label === picked.label)) chips.push(picked);
    }
    return chips;
  }, [activeProposal, current, pickedMediaChips, selectedStem, sourceMediaCount, timelineDuration, timelineSnapshot.cut_boundaries.length]);

  const effectiveContextChips: ContextChip[] = useMemo(() => {
    return realContextChips.filter(
      (chip) => !dismissedContextChips.includes(`${chip.kind ?? "tag"}:${chip.label}`),
    );
  }, [dismissedContextChips, realContextChips]);

  const realTaskProgress = useMemo(() => {
    const latestJob = activeJobs[activeJobs.length - 1];
    if (latestJob) {
      return {
        label: `${jobKindLabel(latestJob.job_kind)} · ${latestJob.status}`,
        progress: latestJob.percent ?? undefined,
      };
    }
    if (running) {
      return { label: "Agent working..." };
    }
    if (activeProposal) {
      return {
        label: "Proposal awaiting review",
        progress: activeProposal.diffHints.length > 0 ? 100 : undefined,
      };
    }
    return undefined;
  }, [activeJobs, activeProposal, running]);

  const realIndexingMedia: IndexingMediaItem[] = useMemo(() => {
    const importBusy = activeJobs.some((job) => job.job_kind === "local_import" || job.job_kind === "url_import");
    const transcodeJob = activeJobs.find((job) => job.job_kind === "transcode");
    const failedProxySources = failedTranscodeSourceNames(items);
    const pendingProxyAssets = pendingProxyAssetIdSet(timelineSnapshot);
    if (sources.length > 0) {
      return sources.map((source) => {
        const sourceStem = source.name.replace(/\.[^.]+$/, "");
        const proxy = proxies.find((entry) => entry.stem.startsWith(`${sourceStem}-`));
        const readinessEntry = findMediaReadinessEntry(mediaReadiness, source);
        const readiness = readinessEntry ? mediaReadinessUi(readinessEntry) : null;
        const proxyPending = pendingProxyAssets.has(source.id);
        const proxyFailed = proxyPending && failedProxySources.has(source.name);
        return {
          id: source.id,
          assetId: source.id,
          title: source.name,
          stem: proxy?.stem ?? sourceStem,
          detail: readiness
            ? `${formatBytes(readinessEntry?.source_size_bytes ?? source.size_bytes)} source · ${readiness.detailSuffix}`
            : proxy
              ? `${formatBytes(source.size_bytes)} source · proxy ready`
              : proxyFailed
                ? `${formatBytes(source.size_bytes)} source · proxy failed`
                : `${formatBytes(source.size_bytes)} source · awaiting proxy/index`,
          status: readiness?.status ??
            (proxy
              ? "indexed"
              : transcodeJob
                ? "processing"
                : proxyFailed
                  ? "failed"
                  : proxyPending
                    ? "processing"
                    : importBusy
                      ? "imported"
                      : "partial"),
          progress: readiness
            ? typeof readiness.progress === "number"
              ? readiness.progress
              : readiness.status === "processing"
                ? transcodeJob?.percent ?? (proxyPending && !proxyFailed ? 0 : undefined)
              : readiness.status === "indexed"
                ? 100
                : undefined
            : transcodeJob?.percent ?? (proxyPending && !proxyFailed ? 0 : undefined),
        };
      });
    }
    return proxies.map((proxy) => ({
      id: proxy.stem,
      title: proxy.stem,
      stem: proxy.stem,
      detail: `${formatBytes(proxy.size_bytes)} proxy · ${proxy.proxy_path.split("/").pop() ?? "media"}`,
      status: transcodeJob ? "processing" : importBusy ? "imported" : "indexed",
      progress: transcodeJob?.percent ?? undefined,
    }));
  }, [activeJobs, items, mediaReadiness, proxies, sources, timelineSnapshot]);

  const loadedTranscript =
    transcriptState?.state === "loaded" ? transcriptState.transcript : null;

  const realIndexingTasks: IndexingTask[] = useMemo(() => {
    const globalIndexJob = activeJobs.find((job) => job.job_kind === "indexing");
    // Per-signal mapping to a dedicated JobKind. Signals not listed
    // here (face, color, speaker, captions) don't have a standalone
    // background job — they're produced as part of the global
    // "indexing" run, so when that's active they should read as
    // queued, not missing.
    const specificJobs: Partial<Record<IndexingTask["kind"], JobKind>> = {
      scenes: "thumbnails",
      audio: "waveform",
      motion: "motion",
      silence: "silences",
    };
    return ([
      "transcripts",
      "scenes",
      "audio",
      "face",
      "motion",
      "color",
      "silence",
      "speaker",
      "captions",
    ] satisfies IndexingTask["kind"][]).map((kind) => {
      if (kind === "transcripts") {
        if (loadedTranscript) {
          return {
            id: "real-transcripts",
            kind,
            status: "indexed" as const,
            progress: 100,
            detail: "Transcript sidecar loaded",
          };
        }
        if (indexReadiness?.transcripts) {
          return {
            id: "real-transcripts",
            kind,
            status: "indexed" as const,
            progress: 100,
            detail: "Found local transcript output",
          };
        }
        if (globalIndexJob) {
          return {
            id: "real-transcripts",
            kind,
            status: "queued" as const,
            progress: undefined,
            detail: "Waiting for transcript output",
          };
        }
      }
      const mediaCacheStatus = mediaReadiness
        ? mediaReadinessTaskStatus(mediaReadiness, kind)
        : null;
      if (mediaCacheStatus) {
        return {
          id: `real-${kind}`,
          kind,
          status: mediaCacheStatus.status,
          progress: mediaCacheStatus.progress ?? (mediaCacheStatus.status === "indexed" ? 100 : undefined),
          detail: mediaCacheStatus.detail,
        };
      }
      if (indexReadiness && indexTaskReady(indexReadiness, kind)) {
        return {
          id: `real-${kind}`,
          kind,
          status: "indexed" as const,
          progress: 100,
          detail: "Found local index output",
        };
      }
      const jobKind = specificJobs[kind];
      const runningJob = jobKind ? activeJobs.find((job) => job.job_kind === jobKind) : undefined;
      const completed = jobKind ? completedJobKinds.has(jobKind) : false;
      // Precedence: a dedicated job for this signal wins, then a
      // completed-from-local-state marker, then the global indexer
      // (which queues every remaining signal behind it). Only fall
      // through to "missing" when no indexer is doing anything on
      // our behalf — that's the real "user must click Run" state.
      let status: MediaIndexingStatus;
      let detail: string;
      if (runningJob) {
        status = "indexing";
        detail = runningJob.status;
      } else if (completed) {
        status = "indexed";
        detail = "Completed from local job state";
      } else if (globalIndexJob) {
        status = "queued";
        detail = "Waiting for index output";
      } else {
        status = "missing";
        detail = "Not yet run";
      }
      return {
        id: `real-${kind}`,
        kind,
        status,
        progress: runningJob?.percent ?? (completed ? 100 : undefined),
        detail,
      };
    });
  }, [activeJobs, completedJobKinds, hasImportedMedia, indexReadiness, loadedTranscript, mediaReadiness]);

  const realIndexingReady = realIndexingTasks.some((task) => task.status === "indexed");
  const realIndexingStructure: IndexingStructurePreview | undefined = useMemo(() => {
    if (sourceMediaCount === 0) return undefined;
    const duration = timelineDuration > 0
      ? timelineDuration
      : loadedTranscript?.segments.reduce((max, segment) => Math.max(max, segment.end_s), 0) ?? 0;
    const speakers = loadedTranscript
      ? loadedTranscript.speakers.length || new Set(loadedTranscript.segments.map((segment) => segment.speaker_id).filter(Boolean)).size
      : undefined;
    // Scenes: prefer the editorial cut_boundaries (intent-tagged transitions)
    // when present; otherwise fall back to the raw scenedetect shot count.
    // When scenedetect has run but found zero cuts (a single uninterrupted
    // shot), show 0 — distinct from "—" which means "indexer has not run".
    const editorialScenes = timelineSnapshot.cut_boundaries.length;
    const scenes = editorialScenes > 0
      ? editorialScenes
      : indexReadiness?.scenes
        ? indexReadiness.scene_count
        : undefined;
    return {
      duration: duration > 0 ? formatDuration(duration) : undefined,
      scenes,
      segments: loadedTranscript?.segments.length,
      speakers,
      transcriptPercent: indexReadiness?.transcripts ? 100 : undefined,
    };
  }, [indexReadiness?.scenes, indexReadiness?.scene_count, indexReadiness?.transcripts, loadedTranscript, sourceMediaCount, timelineDuration, timelineSnapshot.cut_boundaries.length]);

  const realDeliveryTargets: DeliveryTarget[] = useMemo(
    () => [
      { key: "youtube", active: timelineDuration > 0 },
      { key: "tiktok", active: false },
      { key: "instagram", active: false },
      { key: "captions", active: completedJobKinds.has("indexing") },
      { key: "cover", active: false },
      { key: "custom", active: false },
    ],
    [completedJobKinds, timelineDuration],
  );

  const selectedDeliveryTargets = useDeliveryTargetsStore((s) => s.selected);
  const effectiveDeliveryTargets: DeliveryTarget[] = useMemo(
    () =>
      realDeliveryTargets.map((target) => ({
        ...target,
        active: selectedDeliveryTargets.has(target.key as DeliveryTargetKey),
      })),
    [selectedDeliveryTargets, realDeliveryTargets],
  );

  const realPreflightFindings: PreflightFinding[] = useMemo(() => {
    if (timelineSnapshot.preview_limitations.length > 0) {
      return timelineSnapshot.preview_limitations.map((limitation, index) => ({
        id: `preview-limitation-${index}`,
        severity: "warning",
        message: limitation.message,
        asset: limitation.kind,
        suggestedFix: "Review final render output before delivery.",
      }));
    }
    if (timelineDuration > 0) {
      return [
        {
          id: "timeline-ready",
          severity: "pass",
          message: "Timeline is ready for delivery preflight",
          asset: `${formatDuration(timelineDuration)} timeline`,
        },
      ];
    }
    return [];
  }, [timelineDuration, timelineSnapshot.preview_limitations]);

  const realDeliverySummary: DeliveryRenderSummary | undefined = useMemo(() => {
    if (timelineDuration <= 0) return undefined;
    return {
      duration: formatDuration(timelineDuration),
      outputs: effectiveDeliveryTargets.filter((target) => target.active).length,
      confidence: realPreflightFindings.some((finding) => finding.severity === "warning") ? 0.72 : 0.9,
    };
  }, [effectiveDeliveryTargets, realPreflightFindings, timelineDuration]);

  const previewChanges: PreviewChange[] = useMemo(() => {
    if (!activeProposal) return [];
    // Until backend ships per-cut surfaces, render one chip per diff hint.
    return activeProposal.diffHints.map((hint, i) => ({
      id: `${activeProposal.callId}-${i}`,
      index: i + 1,
      kind: "pending" as const,
      timeS: estimateTimeForHint(hint, activeProposal.snapshot.duration_s, i, activeProposal.diffHints.length),
    }));
  }, [activeProposal]);

  // Stage routing — Edit is the main working surface. Proposal preview,
  // transcript review, evidence, and revisions all live inside this stage.
  const isEditStage = stage === "edit";

  useEffect(() => {
    if (!isEditStage) {
      setInspectorCollapsed(false);
    }
  }, [isEditStage]);

  useEffect(() => {
    setInspectorCollapsed(false);
    if (activeProposal) {
      setRightPanel("inspector");
    }
    setActivePreviewChangeId(undefined);
  }, [activeProposal?.callId]);

  const effectiveDuration = demoMode ? SCREEN2_DURATION_S : timelineDuration > 0 ? timelineDuration : sourceDurationS;
  const effectiveCurrentTime = demoMode ? SCREEN2_CURRENT_TIME_S : timelineDuration > 0 ? timelineTimeS : sourceCurrentTimeS;
  const effectiveChanges = demoMode ? screen2Changes : previewChanges;
  const effectivePlan = demoMode ? screen2Plan : plan;
  const effectiveInspector = demoMode ? screen2Inspector : inspectorData;
  const isTimelinePreview = !demoMode && timelineDuration > 0;
  const selectedPreviewChangeId = demoMode ? "c07" : activePreviewChangeId;

  // FilmSlate inputs — only meaningful when we're previewing real
  // source media (not the timeline-segmented view and not demo mode).
  // The slate stays hidden in those modes by leaving `slateSourceMedia`
  // undefined. Fields beyond `name` are best-effort: the current media
  // store only carries `name` + `size_bytes`, and source-time duration
  // (`sourceDurationS`) is populated once the <video> reads metadata.
  // Resolution / codec / audio aren't surfaced by the model yet — left
  // undefined so the slate collapses them. TODO(redesign): wire probe
  // metadata once it lives on `SourceMediaEntry`.
  const slateSourceMedia = useMemo(() => {
    if (demoMode || isTimelinePreview) return undefined;
    if (!selectedPreviewMedia) return undefined;
    return {
      name: selectedPreviewMedia.name,
      sizeBytes: selectedSource?.size_bytes,
      durationSec: sourceDurationS > 0 ? sourceDurationS : undefined,
    };
  }, [demoMode, isTimelinePreview, selectedPreviewMedia, selectedSource?.size_bytes, sourceDurationS]);

  // Total indexers reported by the backend snapshot — keep in sync with
  // the boolean fields on `IndexReadinessSnapshot`. Hard-coded rather
  // than derived because the snapshot type is a flat record, not a list.
  const INDEXER_TOTAL = 9;
  const slateIndexing = useMemo(() => {
    if (!indexReadiness) {
      // No snapshot yet — show a calm "preparing" placeholder so the
      // slate doesn't claim more progress than is real.
      return {
        ready: 0.1,
        status: "Preparing media…",
        detail: "Awaiting indexer status",
      };
    }
    const ready = indexReadiness.ready_count;
    const fraction = Math.max(0, Math.min(1, ready / INDEXER_TOTAL));
    const transcriptSegment = indexReadiness.transcripts
      ? " · transcript ready"
      : "";
    return {
      ready: fraction,
      status: fraction < 1 ? "Building proxy…" : "Decoding first frame…",
      detail: `${ready} of ${INDEXER_TOTAL} indexers ready${transcriptSegment}`,
    };
  }, [indexReadiness]);
  const seekPreview = (timeS: number) => {
    if (demoMode) return;
    if (isTimelinePreview) {
      requestTimelineSeek(timeS);
    } else {
      requestSourceSeek(timeS);
    }
  };
  const selectPreviewChange = (change: PreviewChange) => {
    setActivePreviewChangeId(change.id);
    seekPreview(change.timeS);
  };
  const jumpPreviewChange = (direction: -1 | 1) => {
    if (effectiveChanges.length === 0 || demoMode) return;
    const sorted = [...effectiveChanges].sort((a, b) => a.timeS - b.timeS);
    const currentIndex = selectedPreviewChangeId
      ? sorted.findIndex((change) => change.id === selectedPreviewChangeId)
      : -1;
    const fallbackIndex =
      direction > 0
        ? sorted.findIndex((change) => change.timeS > effectiveCurrentTime + 0.01)
        : findLastIndex(sorted, (change) => change.timeS < effectiveCurrentTime - 0.01);
    const nextIndex =
      currentIndex >= 0
        ? Math.max(0, Math.min(sorted.length - 1, currentIndex + direction))
        : fallbackIndex >= 0
          ? fallbackIndex
          : direction > 0
            ? sorted.length - 1
            : 0;
    selectPreviewChange(sorted[nextIndex]);
  };
  const realDeliveryWorkspace = (
    <DeliverySurface
      targets={effectiveDeliveryTargets}
      findings={realPreflightFindings}
      summary={realDeliverySummary}
      onToggleTarget={toggleDeliveryTarget}
      onExportNow={() => void runTimelineExport()}
      onFixIssues={setDeliveryRepair}
      onSavePreset={saveDeliveryPreset}
      onGenerateVariants={generateVariants}
      onAgentRepair={(finding) => {
        void runEngineCommand(
          `Repair this delivery preflight finding before export: ${finding.message}`,
        );
      }}
    />
  );
  // Multi-proposal review used to route through `BatchReviewSurface`,
  // but that component was full of demo placeholders (fake titles,
  // hard-coded constraints, fabricated risk notes) that lied to the
  // user when real proposals landed. Multi-proposal review now flows
  // through the normal Edit-stage layout: the inspector rail handles
  // the active proposal, and the "{n} pending" pill in the header
  // tells the user there are more. A real multi-proposal picker can
  // land later as a small list above the inspector.
  const realWorkspace =
    !demoMode && !hasProject ? (
      <Landing />
    ) : !demoMode && stage === "deliver" ? (
      realDeliveryWorkspace
    ) : !demoMode && stage === "skills" ? (
      <SkillsSurface />
    ) : !demoMode && stage === "history" ? (
      <HistorySurface />
    ) : undefined;

  if (demoMode && demoScreen.standalone && demoScreen.workspace) {
    return <>{demoScreen.workspace}</>;
  }

  // Per-screen demos can override the cockpit's command rail. When a demo
  // doesn't supply one, fall back to the screen2 demo data (or real wiring).
  const railProps = demoMode && demoScreen.commandRail
    ? demoScreen.commandRail
    : {
        contextChips: demoMode ? screen2ContextChips : effectiveContextChips,
        plan: effectivePlan,
        taskProgress: demoMode
          ? { label: "Building proposal...", progress: 62, eta: "00:01:48" }
          : realTaskProgress,
        activity: demoMode ? screen2Activity : activity,
        turns: demoMode ? [] : turns,
        suggestions: demoMode ? screen2Suggestions : [],
        initialDraft: demoMode
          ? "Cut this into a tight 8-minute podcast episode.\nRemove dead air but preserve natural pacing."
          : undefined,
      };
  const agentRail = (
    <CommandRail
      hasProject={hasProject || demoMode}
      running={demoMode ? true : running}
      {...railProps}
      chatSessions={demoMode ? [] : chatSessions}
      activeChatSession={demoMode ? null : activeChatSession}
      chatLoading={chatLoading}
      focused={agentFocusMode}
      onToggleFocus={() => setAgentFocusMode((focused) => !focused)}
      onSelectChatSession={(session) => void selectChatSession(session)}
      onOpenHistory={() => void refreshChatSessions()}
      onNewChat={() => void startNewChat()}
      onSubmit={(command) => void runEngineCommand(command)}
      onCancel={() => {
        if (!isTauri()) return;
        invoke("cancel_turn").catch((e) =>
          console.warn("cancel_turn failed", e),
        );
      }}
      onSuggestion={(action) => void runEngineCommand(action.prompt)}
      onRespondUserInput={async (callId, reply) => {
        if (!isTauri()) return;
        await invoke("respond_user_input", { callId, reply });
      }}
      onRespondApproval={async (callId, decision) => {
        if (!isTauri()) return;
        await invoke("respond_approval", { callId, decision });
      }}
      onRemoveChip={(chip) => dismissContextChip(chip)}
      permissionMode={permissionMode}
      onSetPermissionMode={(mode) => void changePermissionMode(mode)}
      onRenameChat={(session, newTitle) => renameChat(session, newTitle)}
      onDeleteChat={(session) => deleteChat(session)}
      mediaSuggestions={mediaSuggestions}
      onPickMedia={attachMediaPick}
    />
  );
  const workspaceOverride = agentFocusMode ? (
    // Full-window chat. The rail itself draws the sidebar + centered
    // conversation column; we just give it the whole workspace.
    <div className="h-full min-h-0 w-full overflow-hidden">
      {agentRail}
    </div>
  ) : demoMode && demoScreen.workspace ? (
    demoScreen.workspace
  ) : (
    realWorkspace
  );

  // ---- Stage shell nodes (2026 UX) ----------------------------------
  // The cinematic preview hero (bare video, no center-mode tabs) and the
  // timeline strip, reused by StageShell. They consume the same state the
  // old cockpit did.
  const stagePreview = (
    <div className="flex h-full w-full min-h-0 flex-col overflow-hidden">
      <MediaOfflineBanner />
      <PreviewSurface
        proposalName={activeProposal?.summary ?? "Source review"}
        pendingCount={effectiveChanges.length}
        changes={effectiveChanges}
        activeChangeId={selectedPreviewChangeId}
        currentTimeS={effectiveCurrentTime}
        durationS={effectiveDuration}
        isPlaying={isPlaying}
        volume={previewVolume}
        rate={previewRate}
        qualityMode={previewQualityMode}
        viewMode={previewViewMode}
        videoSlot={
          isTimelinePreview ? (
            <SegmentedVideoView chrome={false} volume={previewVolume} rate={previewRate} />
          ) : realPreviewSrc && selectedPreviewMedia ? (
            <RealMediaPreviewSlot
              src={realPreviewSrc}
              label={selectedPreviewMedia.label}
              name={selectedPreviewMedia.name}
              isPlaying={isPlaying}
              volume={previewVolume}
              rate={previewRate}
              seekRequestId={sourceSeekRequestId}
              seekTargetS={sourceSeekTargetS}
              onTime={setSourceTime}
              onDuration={setSourceDuration}
              onPlaying={setMediaPlaying}
              onFirstFrame={() => setHasProxyFrame(true)}
            />
          ) : undefined
        }
        sourceMedia={slateSourceMedia}
        hasProxyFrame={hasProxyFrame}
        indexing={slateIndexing}
        onPlayPause={() => setMediaPlaying(!isPlaying)}
        onSelectChange={selectPreviewChange}
        onPrevCut={() => jumpPreviewChange(-1)}
        onNextCut={() => jumpPreviewChange(1)}
        onSeek={seekPreview}
        onSetVolume={setPreviewVolume}
        onSetRate={setPreviewRate}
        onSetQualityMode={setPreviewQualityMode}
        onSetViewMode={setPreviewViewMode}
        onOpenProposalMenu={() => setInspectorCollapsed(false)}
        onInspectProposal={inspectActiveProposal}
        onReviseProposal={reviseActiveProposal}
        onAcceptProposal={activeProposal ? acceptActiveProposal : undefined}
        onRejectProposal={activeProposal ? rejectActiveProposal : undefined}
        onFullscreen={() => setInspectorCollapsed(false)}
      />
    </div>
  );
  const stageTimeline = (
    <TimelineHybrid
      tab={timelineTab}
      onChangeTab={setTimelineTab}
      viewMode={timelineViewMode}
      onChangeViewMode={setTimelineViewMode}
      durationS={effectiveDuration}
      currentTimeS={effectiveCurrentTime}
      changeCount={effectiveChanges.length}
      audioPeaks={realAudioPeaks}
      contentForTab={{ timeline: <TimelinePane /> }}
    />
  );

  return (
    <>
    <AmbientBackground />
    {STAGE_SHELL && !demoMode ? (
      <StageShell
        hasProject={hasProject}
        landing={<Landing />}
        preview={stagePreview}
        timeline={stageTimeline}
        deliver={realDeliveryWorkspace}
        skills={<SkillsSurface />}
        history={<HistorySurface />}
        stage={stage}
        onStage={setStage}
        onCommand={(text) => void runEngineCommand(text)}
        running={running}
        onCancel={() => {
          if (!isTauri()) return;
          invoke("cancel_turn").catch((e) => console.warn("cancel_turn failed", e));
        }}
        projectLabel={current ? projectName(current) : undefined}
        agentRead={
          hasProject
            ? `${briefPendingCount} proposal${briefPendingCount === 1 ? "" : "s"} ready · ${realIndexingReady ? "indexed" : "indexing"}`
            : undefined
        }
      />
    ) : (
    <AppShell
      // Top chrome (brand, stage tabs, status, share/settings) now lives in
      // `<TopChrome />` mounted by AppShell directly. Task 8 lands IdentityRow;
      // Task 9 brings the workspace/stage row back.
      workspace={workspaceOverride}
      commandRail={
        <LeftWorkspaceRail
          active={leftPanel}
          onChange={setLeftPanel}
          agent={agentRail}
          media={
            <ProjectMediaPanel
              projectName={current ? projectName(current) : undefined}
              sourceCount={sourceMediaCount}
              media={realIndexingMedia}
              ready={realIndexingReady}
              episodes={episodeSummary}
              onImport={() => void chooseAndImportFiles()}
              onImportUrl={() => setShowUrlImport(true)}
              onOpenProject={() => void chooseAndOpenProject()}
              onSelectMedia={(stem) => useMediaStore.getState().select(stem)}
              onPlaceMedia={(assetId) => void placeMediaOnTimeline(assetId)}
              generatedMedia={generatedMedia}
              generatedMediaLoading={generatedMediaLoading}
              generatedMediaError={generatedMediaError}
              onRefreshGeneratedMedia={() => void refreshGeneratedMedia()}
              onPlaceGeneratedMedia={(entry) => void placeGeneratedMediaOnTimeline(entry)}
            />
          }
        />
      }
      preview={
        isEditStage ? (
          <div className="flex h-full w-full min-h-0 flex-col overflow-hidden">
            <CenterModeTabs
              active={centerMode}
              onChange={setCenterMode}
              badges={{ brief: briefPendingCount }}
            />
            {centerMode === "brief" ? (
              <div className="flex-1 min-h-0 overflow-hidden">
                {/* Wave 4 W4.6: the focus controller wired in
                    `installFocusAdapter` above owns the tab switch +
                    entity focus on every "Review →". BriefSurface no
                    longer needs a per-click callback. */}
                <BriefSurface />
              </div>
            ) : centerMode === "timeline" ? (
              <div className="flex-1 min-h-0 overflow-hidden">
                <TimelineHybrid
                  tab={timelineTab}
                  onChangeTab={setTimelineTab}
                  viewMode={timelineViewMode}
                  onChangeViewMode={setTimelineViewMode}
                  durationS={effectiveDuration}
                  currentTimeS={effectiveCurrentTime}
                  changeCount={effectiveChanges.length}
                  audioPeaks={demoMode ? screen2AudioPeaks : realAudioPeaks}
                  contentForTab={demoMode ? undefined : { timeline: <TimelinePane /> }}
                />
              </div>
            ) : isTranscriptFirstProjectType(projectType) ? (
              // Wave 4 W4.4: podcast/interview/tutorial projects land on
              // the transcript-first Source view. The legacy video preview
              // stays reachable through the Video sub-tab inside
              // <TranscriptSource>.
              <div className="flex-1 min-h-0 overflow-hidden">
                <TranscriptSource
                  videoSlot={
                    <div className="flex h-full w-full min-h-0 flex-col overflow-hidden">
                      <MediaOfflineBanner />
                      <PreviewSurface
                        proposalName={activeProposal?.summary ?? "Source review"}
                        pendingCount={effectiveChanges.length}
                        changes={effectiveChanges}
                        activeChangeId={selectedPreviewChangeId}
                        currentTimeS={effectiveCurrentTime}
                        durationS={effectiveDuration}
                        isPlaying={isPlaying}
                        volume={previewVolume}
                        rate={previewRate}
                        qualityMode={previewQualityMode}
                        viewMode={previewViewMode}
                        videoSlot={
                          isTimelinePreview ? (
                            <SegmentedVideoView chrome={false} volume={previewVolume} rate={previewRate} />
                          ) : realPreviewSrc && selectedPreviewMedia ? (
                            <RealMediaPreviewSlot
                              src={realPreviewSrc}
                              label={selectedPreviewMedia.label}
                              name={selectedPreviewMedia.name}
                              isPlaying={isPlaying}
                              volume={previewVolume}
                              rate={previewRate}
                              seekRequestId={sourceSeekRequestId}
                              seekTargetS={sourceSeekTargetS}
                              onTime={setSourceTime}
                              onDuration={setSourceDuration}
                              onPlaying={setMediaPlaying}
                              onFirstFrame={() => setHasProxyFrame(true)}
                            />
                          ) : undefined
                        }
                        sourceMedia={slateSourceMedia}
                        hasProxyFrame={hasProxyFrame}
                        indexing={slateIndexing}
                        onPlayPause={() => setMediaPlaying(!isPlaying)}
                        onSelectChange={selectPreviewChange}
                        onPrevCut={() => jumpPreviewChange(-1)}
                        onNextCut={() => jumpPreviewChange(1)}
                        onSeek={seekPreview}
                        onSetVolume={setPreviewVolume}
                        onSetRate={setPreviewRate}
                        onSetQualityMode={setPreviewQualityMode}
                        onSetViewMode={setPreviewViewMode}
                        onOpenProposalMenu={() => setInspectorCollapsed(false)}
                        onInspectProposal={inspectActiveProposal}
                        onReviseProposal={reviseActiveProposal}
                        onAcceptProposal={activeProposal ? acceptActiveProposal : undefined}
                        onRejectProposal={activeProposal ? rejectActiveProposal : undefined}
                        onFullscreen={() => setInspectorCollapsed(false)}
                      />
                    </div>
                  }
                />
              </div>
            ) : (
              <>
            <MediaOfflineBanner />
            <PreviewSurface
            proposalName={demoMode ? "Podcast Tightening v1" : activeProposal?.summary ?? "Source review"}
            pendingCount={effectiveChanges.length}
            changes={effectiveChanges}
            activeChangeId={selectedPreviewChangeId}
            currentTimeS={effectiveCurrentTime}
            durationS={effectiveDuration}
            isPlaying={demoMode ? false : isPlaying}
            volume={previewVolume}
            rate={previewRate}
            qualityMode={previewQualityMode}
            viewMode={previewViewMode}
            videoSlot={
              demoMode ? (
                <Screen2MediaSlot />
              ) : isTimelinePreview ? (
                <SegmentedVideoView chrome={false} volume={previewVolume} rate={previewRate} />
              ) : realPreviewSrc && selectedPreviewMedia ? (
                <RealMediaPreviewSlot
                  src={realPreviewSrc}
                  label={selectedPreviewMedia.label}
                  name={selectedPreviewMedia.name}
                  isPlaying={isPlaying}
                  volume={previewVolume}
                  rate={previewRate}
                  seekRequestId={sourceSeekRequestId}
                  seekTargetS={sourceSeekTargetS}
                  onTime={setSourceTime}
                  onDuration={setSourceDuration}
                  onPlaying={setMediaPlaying}
                  onFirstFrame={() => setHasProxyFrame(true)}
                />
              ) : undefined
            }
            sourceMedia={slateSourceMedia}
            hasProxyFrame={hasProxyFrame}
            indexing={slateIndexing}
            onPlayPause={() => setMediaPlaying(!isPlaying)}
            onSelectChange={selectPreviewChange}
            onPrevCut={() => jumpPreviewChange(-1)}
            onNextCut={() => jumpPreviewChange(1)}
            onSeek={seekPreview}
            onSetVolume={setPreviewVolume}
            onSetRate={setPreviewRate}
            onSetQualityMode={setPreviewQualityMode}
            onSetViewMode={setPreviewViewMode}
            onOpenProposalMenu={() => setInspectorCollapsed(false)}
            onInspectProposal={inspectActiveProposal}
            onReviseProposal={reviseActiveProposal}
            onAcceptProposal={activeProposal ? acceptActiveProposal : undefined}
            onRejectProposal={activeProposal ? rejectActiveProposal : undefined}
            onFullscreen={() => setInspectorCollapsed(false)}
          />
              </>
            )}
          </div>
        ) : stage === "deliver" ? (
          realDeliveryWorkspace
        ) : (
          <span />
        )
      }
      timeline={
        isEditStage ? (
          // Bottom dock is now the timeline only. Transcript + Vedit
          // moved to the right rail as their own tabs — the editor
          // surface stays calm; reference panels live to the side
          // where they don't compete with the cut for vertical space.
          <TimelineHybrid
            tab={timelineTab}
            onChangeTab={setTimelineTab}
            viewMode={timelineViewMode}
            onChangeViewMode={setTimelineViewMode}
            durationS={effectiveDuration}
            currentTimeS={effectiveCurrentTime}
            changeCount={effectiveChanges.length}
            audioPeaks={demoMode ? screen2AudioPeaks : realAudioPeaks}
            contentForTab={demoMode ? undefined : { timeline: <TimelinePane /> }}
          />
        ) : (
          <span />
        )
      }
      timelineCollapsed={false}
      // Wave 4 W4.8: when the center pane is in Timeline mode the
      // expanded TimelineHybrid renders there — the bottom dock would
      // double up. Hide the dock entirely so the user sees the
      // timeline exactly once. Brief and Source modes keep the dock.
      timelineHidden={isEditStage && centerMode === "timeline"}
      inspector={
        isEditStage && inspectorCollapsed ? (
          <CollapsedInspectorButton
            onOpen={(panel) => {
              setRightPanel(panel);
              setInspectorCollapsed(false);
            }}
          />
        ) : isEditStage ? (
          <RightEditPanel
            active={rightPanel}
            onChange={setRightPanel}
            inspector={
              activeProposal || demoMode ? (
                <ProposalInspector
                  data={effectiveInspector}
                  onAccept={acceptActiveProposal}
                  onReject={rejectActiveProposal}
                  onInspectDeeper={inspectActiveProposal}
                  onRevise={reviseActiveProposal}
                  onAgentRepair={() => {
                    void runEngineCommand("Repair the selected proposal's risky edits before acceptance.");
                  }}
                  onMaximize={() => setInspectorCollapsed(false)}
                  onCollapse={() => setInspectorCollapsed(true)}
                />
              ) : (
                <ClipInspector />
              )
            }
            index={
              <IndexRail
                tasks={realIndexingTasks}
                structurePreview={realIndexingStructure}
                episodes={episodeSummary}
                indexerConfig={indexerConfig}
                activeIndexingStatus={
                  activeJobs.find((job) => job.job_kind === "indexing")?.status
                }
                ready={realIndexingReady}
                onReviewIndexResults={() => {
                  void loadIndexerConfig();
                  void runIndexers();
                }}
                onToggleIndexer={(indexer) => void toggleProjectIndexer(indexer)}
                onOpenConfigPath={openConfigPath}
                onRevealConfigPath={revealConfigPath}
              />
            }
            transcript={<TranscriptView stem={selectedStem} />}
            vedit={<VeditPanel />}
          />
        ) : (
          <span />
        )
      }
      inspectorCollapsed={isEditStage && inspectorCollapsed}
      footer={<Footer demoMode={demoMode} />}
    />
    )}
    {/* The bottom dock used to support a popout mode for transcript /
        vedit; now those panels live in the right rail, so the popout
        is gone. */}
    {showNewProject && (
      <NewProjectForm
        onClose={() => {
          setShowNewProject(false);
          setPendingImportPaths(null);
          setPendingImportUrl(null);
        }}
        onCreated={(path) => {
          void completeNewProject(path);
        }}
      />
    )}
    <SettingsModal />
    <AgentsMdEditor />
    <WelcomeCard />
    {showUrlImport && (
      <div className="modal-backdrop" onClick={() => setShowUrlImport(false)}>
        <div className="modal" onClick={(event) => event.stopPropagation()}>
          <header className="modal-header">
            <h2>Import from URL</h2>
            <button
              className="modal-close"
              onClick={() => setShowUrlImport(false)}
              aria-label="Close"
            >
              ×
            </button>
          </header>
          <div className="modal-body">
            <label className="field">
              <span>Media URL</span>
              <input
                value={urlInput}
                onChange={(event) => setUrlInput(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    submitUrlImport();
                  }
                }}
                placeholder="https://..."
                autoFocus
              />
            </label>
            {!current && (
              <p className="field-hint">
                Create a project after entering the URL; Awidat will import it into the new project.
              </p>
            )}
          </div>
          <footer className="modal-footer">
            <button onClick={() => setShowUrlImport(false)}>Cancel</button>
            <button className="primary" onClick={submitUrlImport} disabled={!urlInput.trim()}>
              Import
            </button>
          </footer>
        </div>
      </div>
    )}
    {commandError && (
      <div className="fixed bottom-4 left-1/2 z-50 max-w-xl -translate-x-1/2 rounded-[var(--radius-md)] border border-[var(--color-danger)] bg-[var(--color-surface-elevated)] px-4 py-3 text-[var(--text-body-sm)] text-[var(--color-danger)] shadow-lg">
        {commandError}
      </div>
    )}
    </>
  );
}

function RealMediaPreviewSlot({
  src,
  label,
  name,
  isPlaying,
  volume,
  rate,
  seekRequestId,
  seekTargetS,
  posterSrc,
  onTime,
  onDuration,
  onPlaying,
  onFirstFrame,
}: {
  src: string;
  label: string;
  name: string;
  isPlaying: boolean;
  volume: number;
  rate: number;
  seekRequestId: number;
  seekTargetS: number;
  posterSrc?: string;
  onTime: (timeS: number) => void;
  onDuration: (durationS: number) => void;
  onPlaying: (playing: boolean) => void;
  /**
   * Fires once per mount when the `<video>` first reports that a
   * frame is decoded and ready to display. Used by the parent to
   * cross-fade out the `FilmSlate` loading overlay. Multiple frame
   * events still fire `setHasPaintedFrame(true)` locally — the
   * parent callback is invoked on every transition since it's
   * idempotent (parent owns its own boolean).
   */
  onFirstFrame?: () => void;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [hasPaintedFrame, setHasPaintedFrame] = useState(false);
  const markPainted = () => {
    setHasPaintedFrame(true);
    onFirstFrame?.();
  };

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (Math.abs(video.currentTime - seekTargetS) > 0.05) {
      video.currentTime = seekTargetS;
    }
  }, [seekRequestId, seekTargetS]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (isPlaying) {
      void video.play().catch(() => onPlaying(false));
    } else {
      video.pause();
    }
  }, [isPlaying, onPlaying]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    video.volume = Math.max(0, Math.min(1, volume));
    video.playbackRate = Math.max(0.0625, Math.min(16, rate));
  }, [rate, volume]);

  // Per-frame time push via requestVideoFrameCallback. The browser's
  // `timeupdate` event only fires ~4 times per second, which makes
  // downstream consumers (transcript active-word highlight, scrubber
  // tick) trail the audio by up to ~250ms. rVFC fires once per
  // rendered video frame (~60Hz) so the highlight stays locked to
  // the audio. `onTimeUpdate` on the <video> below still runs as a
  // safety net in case the browser drops rVFC support.
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    type RVFCMetadata = { mediaTime: number };
    type RVFCVideo = HTMLVideoElement & {
      requestVideoFrameCallback?: (
        cb: (now: number, metadata: RVFCMetadata) => void,
      ) => number;
      cancelVideoFrameCallback?: (id: number) => void;
    };
    const rvfc = video as RVFCVideo;
    if (typeof rvfc.requestVideoFrameCallback !== "function") return;
    let cancelled = false;
    let handle = 0;
    const tick = (_now: number, metadata: RVFCMetadata) => {
      if (cancelled) return;
      onTime(metadata.mediaTime);
      const next = rvfc.requestVideoFrameCallback;
      if (next) handle = next(tick);
    };
    handle = rvfc.requestVideoFrameCallback(tick);
    return () => {
      cancelled = true;
      rvfc.cancelVideoFrameCallback?.(handle);
    };
  }, [onTime, src]);

  return (
    <div className="relative h-full w-full bg-black">
      {posterSrc && !hasPaintedFrame ? (
        <>
          <img
            src={posterSrc}
            alt=""
            className="absolute inset-0 h-full w-full object-cover opacity-35 blur-[2px] scale-105"
          />
          <img
            src={posterSrc}
            alt=""
            className="absolute inset-0 m-auto h-full w-full object-contain"
          />
        </>
      ) : null}
      <video
        ref={videoRef}
        src={src}
        poster={posterSrc}
        preload="metadata"
        className="relative h-full w-full object-contain"
        onLoadedMetadata={(event) => onDuration(event.currentTarget.duration)}
        onLoadedData={markPainted}
        onCanPlay={markPainted}
        onTimeUpdate={(event) => {
          markPainted();
          onTime(event.currentTarget.currentTime);
        }}
        onPlay={() => onPlaying(true)}
        onPause={() => onPlaying(false)}
        onEnded={() => onPlaying(false)}
        onClick={() => onPlaying(!isPlaying)}
      />
      <div className="pointer-events-none absolute left-3 top-3 rounded-[var(--radius-sm)] border border-black/50 bg-black/70 px-2 py-1">
        <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-white/70">
          {label}
        </span>
        <span className="ml-2 font-mono text-[var(--text-caption)] text-white/90">{name}</span>
      </div>
    </div>
  );
}


function findLastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (predicate(items[i])) return i;
  }
  return -1;
}

function LeftWorkspaceRail({
  active,
  onChange,
  agent,
  media,
}: {
  active: "agent" | "media";
  onChange: (panel: "agent" | "media") => void;
  agent: ReactNode;
  media: ReactNode;
}) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <PanelSwitch
        value={active}
        options={[
          { value: "agent", label: "Agent" },
          { value: "media", label: "Media" },
        ]}
        onChange={onChange}
      />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {active === "agent" ? agent : media}
      </div>
    </div>
  );
}

type RightPanelKey = "inspector" | "index" | "transcript" | "vedit";

function RightEditPanel({
  active,
  onChange,
  inspector,
  index,
  transcript,
  vedit,
}: {
  active: RightPanelKey;
  onChange: (panel: RightPanelKey) => void;
  inspector: ReactNode;
  index: ReactNode;
  transcript: ReactNode;
  vedit: ReactNode;
}) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <PanelSwitch
        value={active}
        options={[
          { value: "inspector", label: "Inspector" },
          { value: "index", label: "Index" },
          { value: "transcript", label: "Transcript" },
          { value: "vedit", label: "Vedit" },
        ]}
        onChange={onChange}
      />
      {/* Transcript owns its own scroll (virtualized list); the other
       *  tabs are short forms / commit logs that scroll the whole pane.
       *  Transcript variant needs `display: flex` so the .transcript-pane
       *  child's `flex: 1` resolves to the available height — without it
       *  the pane collapses to natural content height and the inner
       *  .transcript-scroll has nothing to scroll within. */}
      <div
        className={
          active === "transcript"
            ? "min-h-0 flex-1 flex flex-col overflow-hidden"
            : "min-h-0 flex-1 overflow-y-auto"
        }
      >
        {active === "inspector"
          ? inspector
          : active === "index"
            ? index
            : active === "transcript"
              ? transcript
              : vedit}
      </div>
    </div>
  );
}

function PanelSwitch<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-0 border-b border-[var(--color-border-subtle)] px-2 py-1">
      {options.map((option) => {
        const selected = value === option.value;
        return (
          <button
            key={option.value}
            type="button"
            onClick={() => onChange(option.value)}
            className={[
              "h-7 flex-1 rounded-[var(--radius-sm)] px-2 text-[var(--text-caption)] font-medium transition-colors",
              selected
                ? "bg-[var(--color-surface-selected)] text-[var(--color-text-primary)]"
                : "text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
            ].join(" ")}
            aria-pressed={selected}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

function ProjectMediaPanel({
  projectName,
  sourceCount,
  media,
  ready,
  episodes,
  onImport,
  onImportUrl,
  onOpenProject,
  onSelectMedia,
  onPlaceMedia,
  generatedMedia,
  generatedMediaLoading,
  generatedMediaError,
  onRefreshGeneratedMedia,
  onPlaceGeneratedMedia,
}: {
  projectName?: string;
  sourceCount: number;
  media: IndexingMediaItem[];
  ready: boolean;
  episodes?: IndexingEpisodeSummary;
  onImport: () => void;
  onImportUrl: () => void;
  onOpenProject: () => void;
  onSelectMedia: (stem: string) => void;
  onPlaceMedia: (assetId: string) => void;
  generatedMedia: GeneratedMediaEntry[];
  generatedMediaLoading: boolean;
  generatedMediaError: string | null;
  onRefreshGeneratedMedia: () => void;
  onPlaceGeneratedMedia: (entry: GeneratedMediaEntry) => void;
}) {
  const indexedCount = media.filter((item) => item.status === "indexed").length;
  const activeCount = media.filter((item) => item.status === "indexing" || item.status === "processing" || item.status === "partial").length;
  const mediaState = sourceCount === 0
    ? "no media"
    : ready
      ? "agent usable"
      : activeCount > 0
        ? "indexing"
        : "needs index";
  return (
    <Stack gap="4" className="p-3">
      <Stack gap="1">
        <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          Project media
        </span>
        <span className="text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
          {projectName ?? "No project"}
        </span>
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          {sourceCount} source {sourceCount === 1 ? "item" : "items"} · {mediaState}
        </span>
      </Stack>
      <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
        <Button variant="secondary" size="sm" onClick={onImport} className="justify-center">
          Add files
        </Button>
        <Button variant="ghost" size="sm" onClick={onImportUrl}>
          Add URL
        </Button>
      </div>
      <Inline justify="between" align="center" gap="2">
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          {indexedCount} indexed · {activeCount} active
        </span>
        <Button variant="ghost" size="sm" onClick={onOpenProject}>
          Change project
        </Button>
      </Inline>
      {episodes && episodes.total > 0 ? (
        <Card padding="sm" tone="flat">
          <Stack gap="2">
            <Inline justify="between" align="baseline" gap="2">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Episodes
              </span>
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                {episodes.total} detected
              </span>
            </Inline>
            <Inline gap="1" wrap="wrap">
              <span className="rounded-md px-1.5 py-0.5 text-[10px] font-medium bg-[rgba(74,200,130,0.16)] text-[#5EEAD4]">
                {episodes.accepted} accepted
              </span>
              <span className="rounded-md px-1.5 py-0.5 text-[10px] font-medium bg-[rgba(217,165,75,0.16)] text-[#FCD34D]">
                {episodes.reviewNeeded} review
              </span>
              {episodes.rejected > 0 ? (
                <span className="rounded-md px-1.5 py-0.5 text-[10px] font-medium bg-[rgba(220,100,95,0.16)] text-[#FCA5A5]">
                  {episodes.rejected} rejected
                </span>
              ) : null}
            </Inline>
            <Stack gap="1">
              {episodes.episodes.slice(0, 3).map((episode) => (
                <Inline key={episode.id} justify="between" align="center" gap="2">
                  <span className="min-w-0 truncate text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                    {episode.name || `Episode ${episode.order}`}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-[var(--color-text-muted)]">
                    {formatDuration(episode.durationS)}
                  </span>
                </Inline>
              ))}
            </Stack>
          </Stack>
        </Card>
      ) : null}
      <GeneratedMediaPanel
        entries={generatedMedia}
        loading={generatedMediaLoading}
        error={generatedMediaError}
        onRefresh={onRefreshGeneratedMedia}
        onUse={onPlaceGeneratedMedia}
      />
      <Stack gap="2">
        {media.length > 0 ? media.map((item) => (
          <div
            key={item.id}
            role="button"
            tabIndex={0}
            onClick={() => {
              if (item.stem) onSelectMedia(item.stem);
            }}
            onKeyDown={(event) => {
              if (event.key !== "Enter" && event.key !== " ") return;
              event.preventDefault();
              if (item.stem) onSelectMedia(item.stem);
            }}
            className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-3 py-2 text-left transition-colors hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]"
            title={item.title}
            draggable={item.assetId !== undefined}
            onDragStart={(event) => {
              if (!item.assetId) return;
              event.dataTransfer.setData("application/x-awidat-media", item.assetId);
              event.dataTransfer.setData("text/plain", item.assetId);
              event.dataTransfer.effectAllowed = "copy";
            }}
          >
            <Inline justify="between" align="start" gap="2">
              <Stack gap="1" className="min-w-0">
                <span className="truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
                  {item.title}
                </span>
                {item.detail ? (
                  <span className="line-clamp-2 text-[var(--text-caption)] text-[var(--color-text-muted)]">
                    {item.detail}
                  </span>
                ) : null}
                {typeof item.progress === "number" ? (
                  <div className="mt-1 h-1 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
                    <div
                      className="h-full rounded-full bg-[var(--color-processing)]"
                      style={{ width: `${Math.max(0, Math.min(100, item.progress))}%` }}
                    />
                  </div>
                ) : null}
              </Stack>
              <StatusPillFromMapping
                mapping={mediaStatusPill(item.status)}
                label={mediaStatusLabel(item.status)}
                className="shrink-0"
              />
            </Inline>
            {item.assetId ? (
              <div className="mt-2 flex justify-end">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={(event) => {
                    event.stopPropagation();
                    if (item.assetId) onPlaceMedia(item.assetId);
                  }}
                >
                  Add to timeline
                </Button>
              </div>
            ) : null}
          </div>
        )) : (
          <Card padding="sm" tone="flat">
            <Stack gap="2">
              <span className="text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
                No media yet
              </span>
              <span className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-muted)]">
                Add source files here. The edit surface can stay open while indexing catches up.
              </span>
            </Stack>
          </Card>
        )}
      </Stack>
    </Stack>
  );
}

function mediaStatusPill(status: IndexingMediaItem["status"]): StatusPillMapping {
  switch (status) {
    case "indexed":
    case "imported":
      return { family: "job", state: "ready" };
    case "indexing":
      return { family: "job", state: "running" };
    case "processing":
    case "queued":
      // "reviewing" → proposal/proposed (awaiting human / queued for action).
      return { family: "proposal", state: "proposed" };
    case "partial":
      // Lossy: original "warning" → job/failed visually per Task 4 mapping.
      return { family: "job", state: "failed" };
    case "failed":
      return { family: "job", state: "failed" };
    case "missing":
      // "missing" → job/idle.
      return { family: "job", state: "idle" };
  }
}
function mediaStatusLabel(status: IndexingMediaItem["status"]): string {
  switch (status) {
    case "indexed":
      return "Indexed";
    case "imported":
      return "Imported";
    case "indexing":
      return "Indexing";
    case "processing":
      return "Processing";
    case "queued":
      return "Queued";
    case "partial":
      return "Partial";
    case "failed":
      return "Failed";
    case "missing":
      return "Missing";
  }
}
function CollapsedInspectorButton({
  onOpen,
}: {
  onOpen: (panel: "inspector" | "index") => void;
}) {
  return (
    <div className="flex h-full w-full flex-col items-stretch gap-1 py-2">
      <CollapsedRailSpine
        label="Index"
        onOpen={() => onOpen("index")}
      />
      <div className="mx-1 h-px shrink-0 bg-[var(--color-border-subtle)]" aria-hidden />
      <CollapsedRailSpine
        label="Inspector"
        onOpen={() => onOpen("inspector")}
      />
    </div>
  );
}

function CollapsedRailSpine({
  label,
  onOpen,
}: {
  label: string;
  onOpen: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group flex flex-1 min-h-[100px] flex-col items-center justify-center gap-2 px-1 text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]"
      aria-label={`Open ${label}`}
      title={`Open ${label}`}
    >
      <PanelRightOpen className="h-4 w-4 shrink-0 stroke-[1.75] opacity-70 group-hover:opacity-100" />
      <span
        className="font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--text-caption)]"
        style={{ writingMode: "vertical-rl" }}
      >
        {label}
      </span>
    </button>
  );
}

// `NoProjectWorkspace` was removed in Task 11 (redesign empty state). Its
// replacement is `<Landing />` in `./shell/empty/Landing.tsx`, rendered
// via `realWorkspace` above.

/**
 * Footer — thin wrapper that delegates to the redesigned
 * `shell/chrome/Footer` (Task 10). The `demoMode` prop is retained for the
 * call site but is currently a no-op; the redesigned footer renders the
 * same job/system state in both demo and live runs.
 */
function Footer(_: { demoMode?: boolean }) {
  return <ChromeFooter />;
}

function projectName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function stemFromPath(path: string | undefined): string | null {
  if (!path) return null;
  const normalized = path.replace(/\\/g, "/");
  const file = normalized.split("/").pop() ?? normalized;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file;
}

function formatDuration(totalSeconds: number): string {
  const safe = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const seconds = safe % 60;
  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, "0")}:${seconds.toString().padStart(2, "0")}`;
  }
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const precision = unit === 0 || value >= 10 ? 0 : 1;
  return `${value.toFixed(precision)} ${units[unit]}`;
}

type IndexReadinessSnapshot = {
  transcripts: boolean;
  scenes: boolean;
  audio: boolean;
  face: boolean;
  motion: boolean;
  color: boolean;
  silence: boolean;
  speaker: boolean;
  captions: boolean;
  ready_count: number;
  scene_count: number;
};

type ProjectEpisodesResponse = {
  total: number;
  accepted: number;
  review_needed: number;
  rejected: number;
  episodes: Array<{
    id: string;
    name: string;
    order: number;
    asset_id: string;
    start_s: number;
    end_s: number;
    duration_s: number;
    confidence: number;
    status: "accepted" | "review_needed" | "rejected";
    evidence_count: number;
  }>;
};

function projectEpisodesToIndexingSummary(
  response: ProjectEpisodesResponse,
): IndexingEpisodeSummary {
  return {
    total: response.total,
    accepted: response.accepted,
    reviewNeeded: response.review_needed,
    rejected: response.rejected,
    episodes: response.episodes.map((episode) => ({
      id: episode.id,
      name: episode.name,
      order: episode.order,
      startS: episode.start_s,
      endS: episode.end_s,
      durationS: episode.duration_s,
      confidence: episode.confidence,
      status: episode.status,
      evidenceCount: episode.evidence_count,
    })),
  };
}

function indexTaskReady(
  readiness: IndexReadinessSnapshot,
  kind: IndexingTask["kind"],
): boolean {
  return readiness[kind];
}

function mediaReadinessTaskStatus(
  readiness: MediaReadinessSnapshot,
  kind: IndexingTask["kind"],
): { status: MediaIndexingStatus; detail: string; progress?: number } | null {
  const transcriptProgress = speechProgressForTask(readiness, kind);
  if (transcriptProgress) {
    return {
      status: "indexing",
      detail: transcriptProgress.label,
      progress: transcriptProgress.percent ?? undefined,
    };
  }
  const cacheKey = mediaCacheKeyForTask(kind);
  if (!cacheKey || readiness.entries.length === 0) return null;
  const statuses = readiness.entries.map((entry) => entry.cache[cacheKey]);
  if (statuses.some((status) => status === "failed")) {
    return { status: "failed", detail: "Media service reported cache failure" };
  }
  if (statuses.some((status) => status === "pending")) {
    return { status: "indexing", detail: "Media service is building this signal" };
  }
  if (statuses.every((status) => status === "ready" || status === "skipped")) {
    return { status: "indexed", detail: "Found media-service output" };
  }
  if (statuses.some((status) => status === "ready" || status === "stale")) {
    return { status: "partial", detail: "Some media-service output is available" };
  }
  if (statuses.every((status) => status === "unsupported")) {
    return { status: "missing", detail: "Unsupported for this media" };
  }
  return null;
}

function speechProgressForTask(
  readiness: MediaReadinessSnapshot,
  kind: IndexingTask["kind"],
): MediaReadinessSnapshot["entries"][number]["transcript_progress"] | null {
  if (kind !== "transcripts" && kind !== "captions" && kind !== "speaker") {
    return null;
  }
  return readiness.entries.find((entry) => entry.transcript_progress)?.transcript_progress ?? null;
}

function mediaCacheKeyForTask(
  kind: IndexingTask["kind"],
): keyof MediaCacheReadiness | null {
  switch (kind) {
    case "transcripts":
      return "transcript";
    case "captions":
      return "captions";
    case "scenes":
      return "scenes";
    case "audio":
      return "audio_analysis";
    case "face":
      return "face_detection";
    case "motion":
      return "motion_analysis";
    case "color":
      return "color_analysis";
    case "silence":
      return "silence_detection";
    case "speaker":
      return null;
  }
}

function jobKindLabel(kind: string): string {
  return kind
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function firstTimelineSidecars(snapshot: TimelineSnapshot): {
  thumbnailDir: string | null;
  waveformPath: string | null;
} {
  let thumbnailDir: string | null = null;
  let waveformPath: string | null = null;
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      thumbnailDir ??= item.thumbnail_dir;
      waveformPath ??= item.waveform_path;
      if (thumbnailDir && waveformPath) return { thumbnailDir, waveformPath };
    }
  }
  return { thumbnailDir, waveformPath };
}

function sampleEvenly<T>(items: T[], maxItems: number): T[] {
  if (items.length <= maxItems) return items;
  if (maxItems <= 0) return [];
  return Array.from({ length: maxItems }, (_, index) => {
    const sourceIndex = Math.floor((index / Math.max(1, maxItems - 1)) * (items.length - 1));
    return items[sourceIndex];
  });
}

function downsamplePeaks(peaks: number[], targetCount: number): number[] {
  if (peaks.length <= targetCount) return peaks.map(normalizePeak);
  const bucketSize = peaks.length / targetCount;
  return Array.from({ length: targetCount }, (_, index) => {
    const start = Math.floor(index * bucketSize);
    const end = Math.max(start + 1, Math.floor((index + 1) * bucketSize));
    let max = 0;
    for (let i = start; i < end && i < peaks.length; i += 1) {
      max = Math.max(max, Math.abs(peaks[i] ?? 0));
    }
    return normalizePeak(max);
  });
}

function normalizePeak(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0.04, Math.min(1, Math.abs(value)));
}

function pendingProxyAssetIdSet(snapshot: TimelineSnapshot): Set<string> {
  const ids = new Set<string>();
  for (const track of snapshot.tracks ?? []) {
    if (track.kind !== "video") continue;
    for (const item of track.items ?? []) {
      if (item.kind !== "clip") continue;
      if (!item.asset_id || item.duration_s <= 0) continue;
      if (item.proxy_path !== null) continue;
      if (item.playable_kind === "proxy") continue;
      ids.add(item.asset_id);
    }
  }
  return ids;
}

function failedTranscodeSourceNames(items: AnyAgentItem[]): Set<string> {
  const names = new Set<string>();
  for (const item of items) {
    if (
      item.kind !== "job" ||
      item.job_kind !== "transcode" ||
      item.phase !== "completed" ||
      item.result === null ||
      typeof item.result !== "object" ||
      !("err" in item.result)
    ) {
      continue;
    }
    const match = item.status.match(/^transcode (.+): /);
    if (match?.[1]) {
      names.add(match[1]);
    }
  }
  return names;
}

type AnyAgentItem = ReturnType<typeof useAgentStore.getState>["items"][number];
type ToolCallItem = Extract<AnyAgentItem, { kind: "tool_call" }>;
type JobItem = Extract<AnyAgentItem, { kind: "job" }>;
type ActivitySourceItem = JobItem;

type ChatHistory = {
  session: ChatSessionSummary | null;
  items: Item[];
};

function activityFor(item: ActivitySourceItem): ActivityEntry {
  const id = item.id.toString();
  const status = summarizeJobStatus(item.status);
  return {
    id,
    timestamp: shortTime(),
    text: `${jobKindLabel(item.job_kind)} · ${status.summary}`,
    detail: status.detail,
    kind: "result",
  };
}

/** Short human one-liner for a tool call inside the conversation rail.
 *  Mirrors the dispatch in ChatStream.tsx's `summarizeToolCall` so the
 *  same vocabulary appears in both surfaces. */
function summarizeToolForRail(item: ToolCallItem): string {
  const args = item.args;
  const record =
    args && typeof args === "object" && !Array.isArray(args)
      ? (args as Record<string, unknown>)
      : null;
  switch (item.name) {
    case "view_timeline":
      return "Read timeline";
    case "view_episode":
      return "Read episode map";
    case "view_frame":
      return typeof record?.at_s === "number"
        ? `Inspected frame at ${record.at_s.toFixed(2)}s`
        : "Inspected frame";
    case "apply_edl":
      return typeof record?.reasoning === "string"
        ? oneLine(record.reasoning as string, 96)
        : "Proposed timeline edit";
    case "find_episode_start":
      return "Found publishable episode start";
    case "podcast_episode_spans":
      return "Planned candidate episode spans";
    case "podcast_edit_proposal":
      return "Built edit proposal";
    case "podcast_apply_accepted_edits":
      return "Prepared accepted edits";
    case "podcast_audio_polish":
      return "Checked audio polish";
    case "podcast_visual_polish":
      return "Checked visual polish";
    case "podcast_qc_report":
      return "Ran podcast QC";
    case "podcast_smooth_cut_boundaries":
      return "Checked cut smoothness";
    case "podcast_post_draft_check":
      return "Checked draft boundaries";
    case "find_beat":
      return typeof record?.kind === "string"
        ? `Found ${record.kind} beats`
        : "Found editorial beats";
    case "inspect_moment":
      return typeof record?.moment_id === "string"
        ? `Inspected ${record.moment_id}`
        : "Inspected moment";
    case "read_index":
      return typeof record?.channel === "string"
        ? `Read ${record.channel} index`
        : "Read index";
    case "start_indexing":
      return "Started indexing";
    case "start_render":
      return "Started render";
    default:
      return item.name.replace(/_/g, " ");
  }
}

function oneLine(text: string, max: number): string {
  const t = text.replace(/\s+/g, " ").trim();
  return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

function summarizeJobStatus(status: string): { summary: string; detail?: string } {
  const firstLine = status.split("\n").find((line) => line.trim().length > 0)?.trim() ?? status;
  const failureCounts = firstLine.match(
    /(\d+)\s+wrote,\s+(\d+)\s+skipped,\s+(\d+)\s+failed,\s+(\d+)\s+dep-skipped/,
  );
  if (failureCounts) {
    const [, wrote, skipped, failed, depSkipped] = failureCounts;
    return {
      summary: `${wrote} wrote, ${failed} failed`,
      detail: `${skipped} skipped, ${depSkipped} dependency-skipped. Open the Index panel for the failing indexers.`,
    };
  }
  if (firstLine.length <= 120) {
    return { summary: firstLine };
  }
  return {
    summary: `${firstLine.slice(0, 96)}...`,
    detail: firstLine.slice(96, 360),
  };
}

function shortTime(): string {
  const d = new Date();
  return `${d.getHours().toString().padStart(2, "0")}:${d.getMinutes().toString().padStart(2, "0")}`;
}

function stepStatus(s: string): PlanItem["status"] {
  if (s === "complete" || s === "done") return "complete";
  if (s === "in_progress" || s === "running") return "in_progress";
  if (s === "failed" || s === "error") return "failed";
  return "pending";
}

// Estimate a time-on-the-timeline for a diff hint when we don't have one in the
// protocol yet. Distributes hints evenly across the timeline duration so the
// jump chips and scrubber dots aren't all stacked at zero.
function estimateTimeForHint(_hint: unknown, durationS: number, i: number, total: number): number {
  if (!durationS || total === 0) return 0;
  return ((i + 1) / (total + 1)) * durationS;
}

export default App;
