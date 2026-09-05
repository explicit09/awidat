/** Montage workspace composition and project lifecycle wiring. */

import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { editorDispatch } from "./editor/tauriDispatch";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { useAgentStore } from "./agent/store";
import { isAuthReadyForAgent } from "./agent/composerAuthGate";
import { buildTurnContext, chatHistoryLoader } from "./agent/turnContext";
import { useProjectStore } from "./app/state";
import { shouldReplaceDeferredChatHistory } from "./app/deferredHydrationGuards";
import { deferNonCriticalHydration } from "./app/startupHydration";
import { AgentsMdEditor } from "./app/AgentsMdEditor";
import { NewProjectForm } from "./app/NewProjectForm";
import { SettingsModal } from "./app/SettingsModal";
import { AuthChooser } from "./app/auth/AuthChooser";
import { WelcomeCard } from "./app/WelcomeCard";
import { useMediaStore } from "./media/store";
import { GeneratedMediaPanel } from "./media/GeneratedMediaPanel";
import { useGeneratedMediaStore, type GeneratedMediaEntry } from "./media/generatedMediaStore";
import { mediaStreamUrl } from "./media/mediaStreamUrl";
import { resumePreviewAudio } from "./media/previewAudioGraph";
import { resolvePreviewMedia, type PreviewQualityMode } from "./media/previewSource";
import { SegmentedVideoView } from "./media/SegmentedVideoView";
import { aspectRatioLabel } from "./media/programFrame";
import { droppedImportPaths } from "./media/dropImportPaths";
import { findMediaReadinessEntry, mediaReadinessUi } from "./media/readiness";
import { useTranscriptStore } from "./transcript/store";
import { TranscriptView } from "./transcript/TranscriptView";
import { VeditPanel } from "./vedit/VeditPanel";
import { NotesPanel } from "./notes/NotesPanel";
import {
  StageShell,
  DeliverySurface,
  DRAFT_METADATA_JOB_ID,
  SkillsSurface,
  HistorySurface,
  IndexRail,
  PreviewInsights,
  ProposalInspector,
  type ChatSessionSummary,
  type ContextChip,
  type MediaSuggestion,
  type DeliveryRenderSummary,
  type DeliveryTarget,
  type IndexingMediaItem,
  type IndexingEpisodeSummary,
  type IndexingStructurePreview,
  type IndexingTask,
  type IndexerConfigSnapshot,
  type PreflightFinding,
  type PreviewChange,
} from "./shell";
import { Landing } from "./shell/empty/Landing";
import { Button, Card, Inline, Stack, StatusPillFromMapping, type MediaIndexingStatus, type StatusPillMapping } from "./ui";
import { ClipInspector } from "./inspector/ClipInspector";
import { stageFromWorkspaceShortcut, useStageStore } from "./state";
import { useAppGlue } from "./state/appGlue";
import { useBriefProposalsStore } from "./state/briefProposals";
import { deriveRanges, installDefaultAdapter as installFocusAdapter } from "./state/focusController";
import { useSettings } from "./state/settings";
import { useAuth } from "./state/auth";
import {
  providerKeyForTarget,
  useUploadPrefs,
} from "./state/uploadPrefs";
import {
  shouldUploadRenderTarget,
  useUploadAccountSelections,
} from "./state/uploadAccountSelections";
import { useUploadMetadata } from "./state/uploadMetadata";
import { useRenderQueueWorker } from "./app/useRenderQueueWorker";
import { SchedulerWorkspace } from "./app/scheduler/SchedulerWorkspace";
import {
  DELIVERY_TARGETS,
  renderQueueLabelForTarget,
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
import type { AgentProfile, Item, JobKind, MediaCacheReadiness, MediaReadinessSnapshot, PermissionMode, TimelineSnapshot } from "./protocol";
import { TIMELINE_CHANGED_EVENT } from "./protocol";
import { StageHarness } from "./media/stage/StageHarness";
import "./ui/tokens.css";
import "./App.css";
import "./ui/glass.css";
import { AmbientBackground } from "./ui/glass";

let optimisticUserInputCounter = 0;

function createOptimisticUserInput(text: string): Extract<Item, { kind: "user_input" }> {
  optimisticUserInputCounter += 1;
  return {
    kind: "user_input",
    id: `optimistic-user-${Date.now()}-${optimisticUserInputCounter}`,
    text,
  };
}

const HELP_DOCS_URL = "https://tadiwa.co/montage/setup";
const HELP_REPORT_ISSUE_URL = "https://github.com/explicit09/awidat/issues/new";
const PERF_ROOT_RENDER_COUNTER =
  import.meta.env.MODE === "perf" ? "__montagePerfAppRootRenderCount" : undefined;

function App() {
  if (PERF_ROOT_RENDER_COUNTER && typeof window !== "undefined") {
    const perfWindow = window as typeof window & Record<string, number | undefined>;
    perfWindow[PERF_ROOT_RENDER_COUNTER] = (perfWindow[PERF_ROOT_RENDER_COUNTER] ?? 0) + 1;
  }
  // Dev-only, Tauri-free harness route for the Stage compositor
  // (Task 8 screenshots this from Playwright). Must short-circuit
  // BEFORE useAppGlue/useRenderQueueWorker or any other Tauri-
  // dependent hook runs, so the route also works in plain Chromium
  // with no Tauri runtime present. The pathname is fixed for the
  // lifetime of a page load, so this is a stable branch — it never
  // flips mid-session and so never changes the hook-call order for
  // a given mount.
  if (typeof window !== "undefined" && window.location.pathname === "/stage-harness") {
    return <StageHarness />;
  }

  // Side effects (Tauri channels, menu routing, project lifecycle).
  useAppGlue();
  // Drive the Deliver-page render queue: drains pending entries one
  // at a time through the appropriate Tauri command. Sits at the
  // root so it survives Deliver-tab unmounts and continues exports
  // when the user switches back to Edit.
  useRenderQueueWorker();
  const current = useProjectStore((s) => s.current);
  const refreshProject = useProjectStore((s) => s.refresh);
  const items = useAgentStore((s) => s.items);
  const running = useAgentStore((s) => s.running);
  const upsertAgentItem = useAgentStore((s) => s.upsert);
  const replaceAgentItems = useAgentStore((s) => s.replace);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setActiveTurnId = useAgentStore((s) => s.setActiveTurnId);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const stage = useStageStore((s) => s.current);
  const setStage = useStageStore((s) => s.set);

  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const timelineSnapshot = useTimelineStore((s) => s.snapshot);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
  const sourceCurrentTimeS = useMediaStore((s) => s.currentTime);
  const sourceDurationS = useMediaStore((s) => s.durationS);
  const activeMediaSize = useMediaStore((s) => s.activeMediaSize);
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
  const pendingProposals = useProposalStore((s) => s.pending);
  const selectProposal = useProposalStore((s) => s.select);
  const inspectorData = useProposalInspectorData();

  useBriefProposalsStore((s) => s.approvals);
  const briefPendingCount = useBriefProposalsStore.getState().pending().length;

  // Connect proposal review to the shared preview and timeline.
  useEffect(() => {
    installFocusAdapter({
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
  }, []);
  const previewQualityMode: PreviewQualityMode = "auto";
  const previewVolume = 0.8;
  const [previewRate, setPreviewRate] = useState(1);
  const [activePreviewChangeId, setActivePreviewChangeId] = useState<string | undefined>(undefined);
  const [realPreviewSrc, setRealPreviewSrc] = useState<string | null>(null);
  const mediaReadinessCommandUnavailableRef = useRef(false);
  const [showNewProject, setShowNewProject] = useState(false);
  const [pendingImportPaths, setPendingImportPaths] = useState<string[] | null>(null);
  const [showUrlImport, setShowUrlImport] = useState(false);
  const [pendingImportUrl, setPendingImportUrl] = useState<string | null>(null);
  const [urlInput, setUrlInput] = useState("");
  const [commandError, setCommandError] = useState<string | null>(null);
  const [indexerConfig, setIndexerConfig] = useState<IndexerConfigSnapshot | undefined>(undefined);
  const [indexReadiness, setIndexReadiness] = useState<IndexReadinessSnapshot | undefined>(undefined);
  const [episodeSummary, setEpisodeSummary] = useState<IndexingEpisodeSummary | undefined>(undefined);
  const [mediaReadiness, setMediaReadiness] = useState<MediaReadinessSnapshot | undefined>(undefined);
  const [runningJobIds, setRunningJobIds] = useState<Set<string> | undefined>(undefined);
  const [chatSessions, setChatSessions] = useState<ChatSessionSummary[]>([]);
  const [activeChatSession, setActiveChatSession] = useState<ChatSessionSummary | null>(null);
  const [chatLoading, setChatLoading] = useState(false);
  const [permissionMode, setPermissionModeState] = useState<PermissionMode>("manual");
  const [agentProfileState, setAgentProfileState] = useState<{
    project: string | null;
    profile: AgentProfile | null;
  }>({ project: null, profile: null });
  const agentProfileRequest = useRef(0);
  const agentProfile = agentProfileState.project === current ? agentProfileState.profile : null;
  // The bottom dock used to host Timeline / Transcript / Vedit as
  // sibling tabs with docked / collapsed / popout chrome. Transcript +
  // Vedit moved to the right rail; the dock is now the timeline only,
  // so the tab + dock-state hooks went with them.

  const hasProject = current !== null;
  const authStatus = useAuth((s) => s.status);
  const refreshAuth = useAuth((s) => s.refresh);
  const openAuth = useAuth((s) => s.open);
  const agentAuthReady = isAuthReadyForAgent(authStatus);

  useEffect(() => {
    void refreshAuth();
  }, [refreshAuth]);
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
        title: "Open Montage project",
      });
      if (typeof picked !== "string") return;
      await invoke("set_project_root", { path: picked });
      await refreshProject();
      setStage("edit");
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
    useDeliveryTargetsStore.getState().clear();
  }

  async function runEngineCommand(command: string) {
    const input = command.trim();
    if (!isTauri() || !input) return;
    if (!agentAuthReady) {
      setCommandError("Sign in to get started");
      openAuth();
      return;
    }
    setCommandError(null);
    setTurnError(null);
    setRunning(true);
    upsertAgentItem(createOptimisticUserInput(input));
    try {
      const turnId = await invoke<string>("start_turn", {
        input,
        context: buildTurnContext(realContextChips),
      });
      setActiveTurnId(turnId);
    } catch (e) {
      if (String(e).includes("turn is already running")) {
        try {
          await invoke("cancel_turn");
          const turnId = await invoke<string>("start_turn", {
            input,
            context: buildTurnContext(realContextChips),
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
    // only vertical/social reframes get YouTube enqueued implicitly as
    // the master.
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
    const accountSelections = useUploadAccountSelections.getState().byProvider;
    // Per-target metadata the user drafted in the form (W5.A3). The
    // form persists under DRAFT_METADATA_JOB_ID; on enqueue we copy
    // the per-provider snapshots onto each entry's own id so the
    // worker can hand them to the backend on render-done.
    const metadataStore = useUploadMetadata.getState();
    const entryIds = Object.fromEntries(
      ordered.map((key) => [key, newQueueId(DELIVERY_TARGETS[key].kind)]),
    ) as Partial<Record<DeliveryTargetKey, string>>;
    const entries: RenderQueueEntry[] = ordered.map((key) => {
      const spec = DELIVERY_TARGETS[key];
      const provider = providerKeyForTarget(key);
      const internal = key === "youtube" && !selected.has("youtube");
      const sourceEntryId =
        !selected.has("youtube") && spec.kind === "video_reframe"
          ? entryIds.youtube
          : undefined;
      const uploadTargets =
        provider && shouldUploadRenderTarget(key, selected, uploadEnabled)
          ? [provider]
          : undefined;
      const uploadAccountIds =
        provider && uploadTargets && accountSelections[provider]
          ? { [provider]: accountSelections[provider] }
          : undefined;
      const entryId = entryIds[key] ?? newQueueId(spec.kind);
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
        label: renderQueueLabelForTarget(key, selected),
        kind: spec.kind,
        internal,
        sourceEntryId,
        status: "pending" as const,
        enqueuedAt: Date.now(),
        reframeWidth: spec.width,
        reframeHeight: spec.height,
        reframeBitrateKbps: spec.videoBitrateKbps,
        stillKind: spec.stillKind,
        uploadTargets,
        uploadAccountIds,
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
    if (!isTauri()) {
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
    if (!isTauri() || !current) {
      setIndexReadiness(undefined);
      return;
    }
    try {
      const snapshot = await invoke<IndexReadinessSnapshot>("index_readiness");
      setIndexReadiness(snapshot);
    } catch (e) {
      console.warn("index_readiness failed", e);
      setIndexReadiness(undefined);
    }
  }

  async function loadProjectEpisodes() {
    if (!isTauri() || !current) {
      setEpisodeSummary(undefined);
      return;
    }
    try {
      const snapshot = await invoke<ProjectEpisodesResponse>("get_project_episodes");
      const summary = projectEpisodesToIndexingSummary(snapshot);
      setEpisodeSummary(summary);
    } catch (e) {
      console.warn("get_project_episodes failed", e);
      setEpisodeSummary(undefined);
    }
  }

  async function loadMediaReadiness() {
    if (!isTauri() || !current) {
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
    if (!isTauri() || !current) {
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

  async function loadInitialChatHistory(args?: {
    scheduledProject: string | null;
    scheduledItemCount: number;
  }) {
    if (!isTauri() || !current) {
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
      const agentState = useAgentStore.getState();
      if (
        !args ||
        shouldReplaceDeferredChatHistory({
          scheduledProject: args.scheduledProject,
          currentProject: useProjectStore.getState().current,
          scheduledItemCount: args.scheduledItemCount,
          currentItemCount: agentState.items.length,
          running: agentState.running,
        })
      ) {
        setActiveChatSession(history.session);
        replaceAgentItems(history.items);
      }
    } catch (e) {
      console.warn("chat history load failed", e);
    } finally {
      setChatLoading(false);
    }
  }

  async function refreshChatSessions() {
    if (!isTauri() || !current) return;
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

  function openConfigPath(path: string) {
    if (!isTauri()) return;
    openPath(path).catch((e) => setCommandError(String(e)));
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
    }
    lastProposalIdRef.current = currentId;
  }, [activeProposal]);

  function inspectActiveProposal() {
    void runEngineCommand("Inspect the selected proposal in detail and list the supporting evidence.");
  }

  function reviseActiveProposal() {
    void runEngineCommand("Revise the selected proposal and explain the tradeoffs.");
  }

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
      } else if (id === MENU_COMMANDS.NAV_PROJECT) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_WORKSPACE) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_MEDIA || id === MENU_COMMANDS.VIEW_MEDIA) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_REVIEW || id === MENU_COMMANDS.VIEW_TIMELINE) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.VIEW_TRANSCRIPT) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_DELIVER) {
        setStage(current ? "deliver" : "edit");
      } else if (id === MENU_COMMANDS.HELP_DOCS) {
        openUrl(HELP_DOCS_URL).catch((e) => setCommandError(String(e)));
      } else if (id === MENU_COMMANDS.HELP_SHORTCUTS) {
        useSettings.getState().open();
      } else if (id === MENU_COMMANDS.HELP_LOGS) {
        invoke("reveal_app_log_dir").catch((e) => setCommandError(String(e)));
      } else if (id === MENU_COMMANDS.HELP_REPORT_ISSUE) {
        openUrl(HELP_REPORT_ISSUE_URL).catch((e) => setCommandError(String(e)));
      }
    });
  });

  // Global keyboard shortcuts: ⌘, (or Ctrl+, on non-mac) opens Settings;
  // ⌘/Ctrl+1..5 jumps between workspace destinations.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const modifierPressed = event.metaKey || event.ctrlKey;
      const isCommaShortcut = event.key === "," && modifierPressed;
      if (isCommaShortcut) {
        event.preventDefault();
        useSettings.getState().open();
        return;
      }
      const nextStage = stageFromWorkspaceShortcut(event.key, modifierPressed);
      if (nextStage) {
        event.preventDefault();
        setStage(nextStage);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setStage]);

  useEffect(() => {
    // Dev-only: when a stage is pinned via VITE_MONTAGE_STAGE (native
    // screenshot tours), don't auto-route the stage back to "edit".
    if (import.meta.env?.VITE_MONTAGE_STAGE) return;

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
  }, [activeProposal, current, hasImportedMedia, setStage, timelineDuration]);

  useEffect(() => {
    if (!isTauri() || !current) {
      clearGeneratedMedia();
      return;
    }
    return deferNonCriticalHydration(() => {
      void refreshGeneratedMedia();
    });
  }, [clearGeneratedMedia, current, refreshGeneratedMedia]);

  useEffect(() => {
    const scheduledProject = current;
    const scheduledItemCount = useAgentStore.getState().items.length;
    return deferNonCriticalHydration(() => {
      void loadInitialChatHistory({ scheduledProject, scheduledItemCount });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current]);

  useEffect(() => {
    if (!running) {
      return deferNonCriticalHydration(() => {
        void refreshChatSessions();
      });
    }
    return undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  useEffect(() => {
    resetSurfaceControls();
  }, [current]);

  useEffect(() => {
    {
      setActiveTranscriptStem(selectedStem);
    }
  }, [selectedStem, setActiveTranscriptStem]);

  useEffect(() => {
    if (!isTauri() || !selectedPreviewMedia) {
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
  }, [selectedPreviewMedia]);

  useEffect(() => {
    if (!isTauri()) {
      setIndexerConfig(undefined);
      return;
    }
    let cancelled = false;
    const cancelDeferred = deferNonCriticalHydration(() => {
      invoke<IndexerConfigSnapshot>("read_indexer_config")
        .then((snapshot) => {
          if (!cancelled) setIndexerConfig(snapshot);
        })
        .catch((e) => {
          console.warn("read_indexer_config failed", e);
          if (!cancelled) setIndexerConfig(undefined);
        });
    });
    return () => {
      cancelled = true;
      cancelDeferred();
    };
  }, [current]);

  // Load persisted agent permission mode so the composer chip shows
  // the right initial value. Falls back to "manual" on error.
  useEffect(() => {
    if (!isTauri()) return;
    return deferNonCriticalHydration(() => {
      invoke<PermissionMode>("get_permission_mode")
        .then((mode) => setPermissionModeState(mode))
        .catch(() => setPermissionModeState("manual"));
    });
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

  useEffect(() => {
    const request = ++agentProfileRequest.current;
    if (!isTauri() || !current) {
      setAgentProfileState({ project: current, profile: "balanced" });
      return;
    }
    const cancel = deferNonCriticalHydration(() => {
      invoke<AgentProfile>("get_agent_profile")
        .then((profile) => {
          if (request === agentProfileRequest.current) {
            setAgentProfileState({ project: current, profile });
          }
        })
        .catch((error) => {
          if (request === agentProfileRequest.current) {
            setCommandError(`Unable to load agent profile: ${String(error)}`);
          }
        });
    });
    return () => {
      cancel();
      ++agentProfileRequest.current;
    };
  }, [current]);

  async function changeAgentProfile(next: AgentProfile) {
    const previous = agentProfile;
    const request = ++agentProfileRequest.current;
    setAgentProfileState({ project: current, profile: null });
    try {
      if (isTauri()) await invoke("set_agent_profile", { profile: next });
      if (request === agentProfileRequest.current) {
        setAgentProfileState({ project: current, profile: next });
      }
    } catch (error) {
      if (request === agentProfileRequest.current) {
        setAgentProfileState({ project: current, profile: previous });
        setCommandError(`Unable to save agent profile: ${String(error)}`);
      }
    }
  }

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
    setIndexReadiness(undefined);

    setEpisodeSummary(undefined);

    setRunningJobIds(undefined);
    setMediaReadiness(undefined);
  }, [current]);

  useEffect(() => {
    return deferNonCriticalHydration(() => {
      void loadIndexReadiness();
      void loadProjectEpisodes();
      void loadRunningJobIds();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, completedJobKinds.size, activeJobs.length, timelineSnapshot.cut_boundaries.length]);

  useEffect(() => {
    mediaReadinessCommandUnavailableRef.current = false;
    return deferNonCriticalHydration(() => {
      void loadMediaReadiness();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, completedJobKinds.size, activeJobs.length, sources.length, proxies.length]);

  useEffect(() => {
    if (!isTauri() || !current || sourceMediaCount === 0) return;
    const id = window.setInterval(() => {
      void loadMediaReadiness();
      void loadRunningJobIds();
    }, activeJobs.length > 0 ? 2_000 : 5_000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeJobs.length, current, sourceMediaCount]);

  // Re-poll readiness when the user comes back to the window. Covers
  // the case where indexer subprocesses kept writing sidecars while
  // the user was in another app, or where the dispatcher's live event
  // stream was lost (orphaned subprocesses from a previous binary
  // session — the disk state is the source of truth either way).
  useEffect(() => {
    function onFocus() {
      void deferNonCriticalHydration(() => {
        void loadIndexReadiness();
        void loadProjectEpisodes();
        void loadMediaReadiness();
        void loadRunningJobIds();
      });
    }
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current]);

  // Episode metadata lives in OTIO metadata.montage.episodes, not in
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
      return [...prev, {
        label: suggestion.chipLabel,
        kind: "media" as const,
        mediaId: suggestion.id,
        mediaToken: suggestion.token,
      }];
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
      { key: "twitter_x", active: false },
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
    };
  }, [effectiveDeliveryTargets, timelineDuration]);

  const previewChanges: PreviewChange[] = useMemo(() => {
    if (!activeProposal) return [];
    return activeProposal.diffHints.flatMap((hint, i) => {
      const [range] = deriveRanges([hint], timelineSnapshot, activeProposal.snapshot);
      return range ? [{
        id: `${activeProposal.callId}-${i}`, index: i + 1,
        kind: "pending" as const, timeS: range.reviewTimeS ?? range.startS,
      }] : [];
    });
  }, [activeProposal, timelineSnapshot]);

  useEffect(() => {
    setActivePreviewChangeId(undefined);
  }, [activeProposal?.callId]);

  const effectiveDuration = timelineDuration > 0 ? timelineDuration : sourceDurationS;
  const effectiveCurrentTime = timelineDuration > 0 ? timelineTimeS : sourceCurrentTimeS;
  const isTimelinePreview = timelineDuration > 0;

  const seekPreview = (timeS: number) => {

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
    if (previewChanges.length === 0) return;
    const sorted = [...previewChanges].sort((a, b) => a.timeS - b.timeS);
    const currentIndex = activePreviewChangeId
      ? sorted.findIndex((change) => change.id === activePreviewChangeId)
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
    />
  );
  const realSchedulerWorkspace = <SchedulerWorkspace />;
  // Preview and transport share the persistent Stage workspace.
  const stageVideoSlot = isTimelinePreview ? (
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
    />
  ) : null;
  const stageProgress =
    effectiveDuration > 0 ? Math.min(100, (effectiveCurrentTime / effectiveDuration) * 100) : 0;
  const togglePreviewPlayback = () => {
    if (!isPlaying) resumePreviewAudio();
    setMediaPlaying(!isPlaying);
  };
  const stagePreview = (
    <div className="flex h-full w-full min-h-0 flex-col gap-2 overflow-hidden">
      {/* context bar — proposal context left, pending count right */}
      <div className="flex h-7 shrink-0 items-center gap-2 px-0.5">
        <span className="inline-flex h-7 max-w-[60%] items-center gap-2 truncate rounded-full border border-[var(--color-border-subtle)] bg-[rgba(255,255,255,0.045)] px-3 text-[12px] font-semibold text-[var(--color-text-primary)]">
          {activeProposal ? (
            <span
              className="h-1.5 w-1.5 shrink-0 rounded-full"
              style={{ backgroundColor: "rgb(217, 165, 75)", boxShadow: "0 0 8px rgba(217,165,75,.7)" }}
              aria-hidden
            />
          ) : null}
          <span className="truncate">{activeProposal?.summary ?? (current ? projectName(current) : "Preview")}</span>
        </span>
        <span className="ml-auto flex items-center gap-1.5">
          {previewChanges.length > 0 ? (
            <span className="font-mono text-[10.5px] tracking-[0.05em] text-[var(--color-text-muted)]">
              {previewChanges.length} pending
            </span>
          ) : null}
          {activeMediaSize ? (
            <span className="inline-flex h-6 items-center gap-1.5 rounded-md border border-[var(--color-border-subtle)] bg-[rgba(0,0,0,0.35)] px-2 font-mono text-[10px] tracking-[0.04em] text-[var(--color-text-secondary)]">
              <strong className="font-semibold text-[var(--color-text-primary)]">
                {activeMediaSize.width}×{activeMediaSize.height}
              </strong>
              {aspectRatioLabel(activeMediaSize.width, activeMediaSize.height)}
            </span>
          ) : null}
        </span>
      </div>
      {/* program monitor box — sized to the media aspect; the picture
          fills it edge-to-edge (see .preview-monitor-box). */}
      <div className="preview-monitor-box relative min-w-0 overflow-hidden">
        {stageVideoSlot ? (
          <div className="absolute inset-0 [&>*]:h-full [&>*]:w-full">{stageVideoSlot}</div>
        ) : (
          <div className="absolute inset-0 grid place-items-center">
            <div className="text-center">
              <div className="text-[12px] font-semibold tracking-wide text-[var(--color-text-secondary)]">
                {activeJobs.some((job) => job.job_kind === "indexing") ? "Indexing…" : "Preview"}
              </div>
              <div className="mt-1 text-[11px] text-[var(--color-text-muted)]">
                {selectedPreviewMedia?.name ?? "Drop a clip or pick one from media"}
              </div>
            </div>
          </div>
        )}
      </div>
      {/* transport — slim row directly under the picture */}
      <div className="flex h-9 shrink-0 items-center gap-3 px-0.5">
        <button
          onClick={togglePreviewPlayback}
          className="glass-ghost grid h-8 w-8 place-items-center rounded-full text-[12px]"
        >
          {isPlaying ? "❚❚" : "▶"}
        </button>
        <button onClick={() => jumpPreviewChange(-1)} className="text-[12px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]">⤴</button>
        <button onClick={() => jumpPreviewChange(1)} className="text-[12px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]">⤵</button>
        <div
          className="group relative h-1.5 flex-1 cursor-pointer rounded-full bg-[rgba(255,255,255,0.16)]"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            const pct = (e.clientX - r.left) / r.width;
            if (effectiveDuration > 0) seekPreview(Math.max(0, Math.min(1, pct)) * effectiveDuration);
          }}
        >
          <div className="h-full rounded-full" style={{ width: `${stageProgress}%`, background: "#EF4444", boxShadow: "0 0 10px #EF4444" }} />
        </div>
        <span className="font-mono text-[10px] text-[var(--color-text-secondary)]">
          {formatDuration(effectiveCurrentTime)} / {formatDuration(effectiveDuration)}
        </span>
      </div>
      {/* insights — suggestion cards + detection/review queue absorb
          the leftover hero height with real analysis data */}
      <PreviewInsights
        changes={previewChanges}
        activeChangeId={activePreviewChangeId}
        onSelectChange={selectPreviewChange}
        onAcceptProposal={activeProposal ? acceptActiveProposal : undefined}
        onRejectProposal={activeProposal ? rejectActiveProposal : undefined}
      />
    </div>
  );
  // Bare timeline canvas — no Hybrid tabs in the Stage strip. TimelinePane's
  // compact header stays visible for transport, track, and zoom controls.
  const stageTimeline = (
    <div className="stage-timeline h-full w-full overflow-hidden">
      <TimelinePane previewRate={previewRate} onPreviewRate={setPreviewRate} />
    </div>
  );

  const stageMedia = (
    <ProjectMediaPanel
      projectName={current ? projectName(current) : undefined}
      sourceCount={sourceMediaCount}
      media={realIndexingMedia}
      ready={realIndexingReady}
      episodes={episodeSummary}
      onImport={() => void chooseAndImportFiles()}
      onImportFiles={(paths) => void importFiles(paths)}
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
  );
  const stageInspector =
    activeProposal ? (
      <div className="proposal-review-stack">
        {pendingProposals.length > 1 ? (
          <div className="proposal-picker" aria-label="Pending proposals">
            <div className="proposal-picker-header">
              <span>Pending proposals</span>
              <span>{pendingProposals.length}</span>
            </div>
            <div className="proposal-picker-list">
              {pendingProposals.map((proposal, index) => (
                <button
                  key={proposal.callId}
                  type="button"
                  className={
                    proposal.callId === activeProposal?.callId
                      ? "proposal-picker-item is-active"
                      : "proposal-picker-item"
                  }
                  onClick={() => selectProposal(proposal.callId)}
                  title={proposal.summary}
                >
                  <span>{index + 1}</span>
                  <strong>{proposal.summary}</strong>
                </button>
              ))}
            </div>
          </div>
        ) : null}
        <ProposalInspector
          data={inspectorData}
          onAccept={acceptActiveProposal}
          onReject={rejectActiveProposal}
          onInspectDeeper={inspectActiveProposal}
          onRevise={reviseActiveProposal}
          onAgentRepair={() => {
            void runEngineCommand("Repair the selected proposal's risky edits before acceptance.");
          }}
        />
      </div>
    ) : (
      <ClipInspector />
    );
  const stageIndex = (
    <IndexRail
      tasks={realIndexingTasks}
      structurePreview={realIndexingStructure}
      indexerConfig={indexerConfig}
      activeIndexingStatus={activeJobs.find((job) => job.job_kind === "indexing")?.status}
      ready={realIndexingReady}
      onRefreshIndexers={() => {
        void loadIndexerConfig();
        void runIndexers();
      }}
      onOpenConfigPath={openConfigPath}
    />
  );
  const stageTranscript = <TranscriptView stem={selectedStem} />;
  const stageVedit = <VeditPanel />;
  const stageNotes = <NotesPanel />;

  return (
    <>
    <AmbientBackground />
    <StageShell
        hasProject={hasProject}
        landing={<Landing />}
        preview={stagePreview}
        timeline={stageTimeline}
        trackCount={timelineSnapshot.tracks.length}
        tools={{
          media: stageMedia,
          inspector: stageInspector,
          index: stageIndex,
          transcript: stageTranscript,
          vedit: stageVedit,
          notes: stageNotes,
        }}
        autoInspect={activeProposal !== null}
        deliver={realDeliveryWorkspace}
        schedule={realSchedulerWorkspace}
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
        mediaSuggestions={mediaSuggestions}
        onPickMedia={attachMediaPick}
        chatSessions={chatSessions}
        activeChatSession={activeChatSession}
        chatLoading={chatLoading}
        onOpenHistory={chatSessions.length === 0 ? () => void refreshChatSessions() : undefined}
        onSelectChatSession={(session) => void selectChatSession(session)}
        onNewChat={() => void startNewChat()}
        permissionMode={permissionMode}
        onSetPermissionMode={(mode) => void changePermissionMode(mode)}
        agentProfile={agentProfile}
        onSetAgentProfile={(profile) => void changeAgentProfile(profile)}
        projectLabel={current ? projectName(current) : undefined}
        agentRead={
          hasProject
            ? `${briefPendingCount} proposal${briefPendingCount === 1 ? "" : "s"} ready · ${realIndexingReady ? "indexed" : "indexing"}`
            : undefined
        }
    />
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
    <AuthChooser />
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
                Create a project after entering the URL; Montage will import it into the new project.
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
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [hasPaintedFrame, setHasPaintedFrame] = useState(false);
  const markPainted = () => {
    setHasPaintedFrame(true);
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

function ProjectMediaPanel({
  projectName,
  sourceCount,
  media,
  ready,
  episodes,
  onImport,
  onImportFiles,
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
  onImportFiles: (paths: string[]) => void;
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
  const selectedMediaStem = useMediaStore((s) => s.selectedStem);
  const indexedCount = media.filter((item) => item.status === "indexed").length;
  const activeCount = media.filter((item) => item.status === "indexing" || item.status === "processing" || item.status === "partial").length;
  const mediaState = sourceCount === 0
    ? "no media"
    : ready
      ? "agent usable"
      : activeCount > 0
        ? "indexing"
        : "needs index";
  function handleDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    const paths = droppedImportPaths(
      Array.from(event.dataTransfer.files) as Array<{ path?: string }>,
    );
    if (paths.length > 0) onImportFiles(paths);
  }

  return (
    <Stack
      gap="4"
      className="p-3"
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes("Files")) {
          event.preventDefault();
          event.dataTransfer.dropEffect = "copy";
        }
      }}
      onDrop={handleDrop}
    >
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
        <Card padding="sm">
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
              <span className="rounded-full border px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-[0.06em] bg-[rgba(45,212,191,0.16)] border-[rgba(45,212,191,0.3)] text-[#5EEAD4]">
                {episodes.accepted} accepted
              </span>
              <span className="rounded-full border px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-[0.06em] bg-[rgba(245,158,11,0.16)] border-[rgba(245,158,11,0.3)] text-[#FCD34D]">
                {episodes.reviewNeeded} review
              </span>
              {episodes.rejected > 0 ? (
                <span className="rounded-full border px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-[0.06em] bg-[rgba(220,100,95,0.16)] border-[rgba(220,100,95,0.3)] text-[#FCA5A5]">
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
      <div className="stage-media-grid grid grid-cols-2 gap-2">
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
            className="stage-media-item glass-content cursor-pointer overflow-hidden p-0 text-left"
            data-selected={item.stem === selectedMediaStem ? "true" : "false"}
            title={item.title}
            draggable={item.assetId !== undefined}
            onDragStart={(event) => {
              if (!item.assetId) return;
              event.dataTransfer.setData("application/x-montage-media", item.assetId);
              event.dataTransfer.setData("text/plain", item.assetId);
              event.dataTransfer.effectAllowed = "copy";
            }}
          >
            <div className="stage-media-thumb rounded-t-xl">
              {item.thumbnail ? (
                <img
                  src={item.thumbnail}
                  alt=""
                  className="h-full w-full object-cover"
                  draggable={false}
                />
              ) : (
                <div className="stage-media-thumb-fallback">
                  <span>{mediaInitials(item.title)}</span>
                </div>
              )}
              <div className="absolute left-2 top-2">
                <StatusPillFromMapping
                  mapping={mediaStatusPill(item.status)}
                  label={mediaStatusLabel(item.status)}
                />
              </div>
            </div>
            <div className="grid gap-1 p-2.5">
              <div className="min-w-0">
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
              </div>
            </div>
            {item.assetId ? (
              <div className="flex justify-end px-2.5 pb-2.5">
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
          <Card padding="sm" className="stage-media-empty">
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
      </div>
    </Stack>
  );
}

function mediaInitials(title: string): string {
  const base = title.replace(/\.[^.]+$/, "").trim();
  const words = base.split(/[\s_-]+/).filter(Boolean);
  if (words.length >= 2) return `${words[0][0] ?? ""}${words[1][0] ?? ""}`.toUpperCase();
  return (base.slice(0, 2) || "M").toUpperCase();
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

// `NoProjectWorkspace` was removed in Task 11 (redesign empty state). Its
// replacement is `<Landing />` in `./shell/empty/Landing.tsx`, rendered
// via `realWorkspace` above.

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

type ChatHistory = {
  session: ChatSessionSummary | null;
  items: Item[];
};

export default App;
