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
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { Bell, CircleHelp, Film, FolderOpen, Import as ImportIcon, PanelRightOpen, Play, Redo2, Settings as SettingsIcon, Share2, Undo2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import wordmark from "./brand/awidat-wordmark.svg";
import { useAgentStore } from "./agent/store";
import { useProjectStore } from "./app/state";
import { NewProjectForm } from "./app/NewProjectForm";
import { useMediaStore } from "./media/store";
import { mediaStreamUrl } from "./media/mediaStreamUrl";
import { useTranscriptStore } from "./transcript/store";
import {
  AppShell,
  BatchReviewSurface,
  CommandRail,
  DeliverySurface,
  IndexingDashboard,
  LensNav,
  PreviewSurface,
  ProposalInspector,
  ReviewLensSurface,
  ReviseSurface,
  StageIndicator,
  StageStub,
  TimelineHybrid,
  type ActivityEntry,
  type AgentCommand,
  type BatchProposal,
  type ContextChip,
  type DeliveryRenderSummary,
  type DeliveryTarget,
  type IndexingMediaItem,
  type IndexingStructurePreview,
  type IndexingTask,
  type IndexerConfigEntry,
  type IndexerConfigSnapshot,
  type AgentEdit,
  type PlanItem,
  type PreflightFinding,
  type PreviewChange,
  type TimelineTab,
  type TimelineViewMode,
  type TranscriptCell,
  type ReviewTranscriptSegment,
} from "./shell";
import { AgentStatusBadge, Button, Card, IconButton, Inline, Pill, Stack } from "./ui";
import { useLensStore, useStageStore } from "./state";
import { useAppGlue } from "./state/appGlue";
import { useProposalInspectorData } from "./state/proposalAdapter";
import { useTimelineStore } from "./timeline/store";
import { useProposalStore } from "./timeline/proposal";
import { MENU_COMMANDS, emitMenuCommand, onMenuCommand } from "./app/menuCommands";
import type { JobKind, TimelineSnapshot, Transcript } from "./protocol";
import {
  screen2Activity,
  SCREEN2_CURRENT_TIME_S,
  SCREEN2_DURATION_S,
  Screen2MediaSlot,
  screen2AgentEdits,
  screen2AudioPeaks,
  screen2Changes,
  screen2ContextChips,
  screen2Diff,
  screen2Frames,
  screen2Inspector,
  screen2Plan,
  screen2Suggestions,
  screen2Transcript,
} from "./shell/screen2Demo";
import { demoScreens, resolveDemoScreenId } from "./shell/demoScreens";
import "./ui/tokens.css";

function App() {
  // Side effects (Tauri channels, menu routing, project lifecycle).
  useAppGlue();

  const current = useProjectStore((s) => s.current);
  const refreshProject = useProjectStore((s) => s.refresh);
  const items = useAgentStore((s) => s.items);
  const running = useAgentStore((s) => s.running);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const stage = useStageStore((s) => s.current);
  const setStage = useStageStore((s) => s.set);
  const currentLens = useLensStore((s) => s.current);
  const setLens = useLensStore((s) => s.set);

  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const timelineSnapshot = useTimelineStore((s) => s.snapshot);
  const sourceCurrentTimeS = useMediaStore((s) => s.currentTime);
  const sourceDurationS = useMediaStore((s) => s.durationS);
  const timelineTimeS = useMediaStore((s) => s.timelineTime);
  const isPlaying = useMediaStore((s) => s.isPlaying);
  const setSourceTime = useMediaStore((s) => s.setTime);
  const setSourceDuration = useMediaStore((s) => s.setDuration);
  const setMediaPlaying = useMediaStore((s) => s.setPlaying);
  const requestSourceSeek = useMediaStore((s) => s.requestSeek);
  const refreshMedia = useMediaStore((s) => s.refresh);
  const sourceSeekRequestId = useMediaStore((s) => s.seekRequestId);
  const sourceSeekTargetS = useMediaStore((s) => s.seekTargetS);
  const sources = useMediaStore((s) => s.sources);
  const proxies = useMediaStore((s) => s.proxies);
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const setActiveTranscriptStem = useTranscriptStore((s) => s.setActiveStem);
  const transcriptState = useTranscriptStore((s) =>
    selectedStem ? s.byStem[selectedStem] : undefined,
  );

  const activeProposal = useProposalStore((s) => s.active);
  const inspectorData = useProposalInspectorData();

  const [timelineTab, setTimelineTab] = useState<TimelineTab>("timeline");
  const [timelineViewMode, setTimelineViewMode] = useState<TimelineViewMode>("proposed");
  const [realVideoFrames, setRealVideoFrames] = useState<string[]>([]);
  const [realAudioPeaks, setRealAudioPeaks] = useState<number[]>([]);
  const [realPreviewSrc, setRealPreviewSrc] = useState<string | null>(null);
  const [showNewProject, setShowNewProject] = useState(false);
  const [pendingImportPaths, setPendingImportPaths] = useState<string[] | null>(null);
  const [showUrlImport, setShowUrlImport] = useState(false);
  const [pendingImportUrl, setPendingImportUrl] = useState<string | null>(null);
  const [urlInput, setUrlInput] = useState("");
  const [commandError, setCommandError] = useState<string | null>(null);
  const [dismissedContextChips, setDismissedContextChips] = useState<string[]>([]);
  const [deliveryTargetOverrides, setDeliveryTargetOverrides] = useState<Record<string, boolean>>({});
  const [indexerConfig, setIndexerConfig] = useState<IndexerConfigSnapshot | undefined>(undefined);
  const [inspectorCollapsed, setInspectorCollapsed] = useState(false);

  const hasProject = current !== null;
  const demoMode = !hasProject && !isTauri();
  const demoScreenId = demoMode
    ? typeof window !== "undefined" && window.location.pathname === "/design/concept"
      ? "screen1"
      : resolveDemoScreenId(typeof window === "undefined" ? "" : window.location.search)
    : "screen2";
  const demoScreen = demoScreens[demoScreenId];
  const selectedProxy = useMemo(
    () => proxies.find((proxy) => proxy.stem === selectedStem) ?? proxies[0] ?? null,
    [proxies, selectedStem],
  );
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
      await invoke("import_locals", { srcPaths: paths, link: false });
      await refreshMedia();
      setStage("indexing");
      setLens("index");
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function importUrl(url: string) {
    const trimmed = url.trim();
    if (!trimmed) return;
    setCommandError(null);
    try {
      await invoke("import_url", { url: trimmed });
      await refreshMedia();
      setStage("indexing");
      setLens("index");
    } catch (e) {
      setCommandError(String(e));
    }
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
      setStage("intent");
      setLens("import");
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function completeNewProject(path: string) {
    await refreshProject().catch(() => {});
    setShowNewProject(false);
    setStage("intent");
    setLens("import");
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
    setDeliveryTargetOverrides({});
  }

  function dismissContextChip(chip: ContextChip) {
    const key = `${chip.kind ?? "tag"}:${chip.label}`;
    setDismissedContextChips((previous) =>
      previous.includes(key) ? previous : [...previous, key],
    );
  }

  async function runEngineCommand(command: string) {
    const input = command.trim();
    if (!isTauri() || !input) return;
    setCommandError(null);
    setTurnError(null);
    setRunning(true);
    try {
      await invoke("start_turn", { input });
    } catch (e) {
      if (String(e).includes("turn is already running")) {
        try {
          await invoke("cancel_turn");
          await invoke("start_turn", { input });
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
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function runTimelineExport() {
    if (!isTauri()) return;
    setCommandError(null);
    try {
      await invoke("start_timeline_render");
    } catch (e) {
      setCommandError(String(e));
    }
  }

  async function acceptAllProposals() {
    if (!isTauri()) return;
    const commandTargets = realBatchProposals.slice();
    await Promise.all(
      commandTargets.map((proposal) =>
        invoke("accept_proposal", { callId: proposal.id }).catch((e) =>
          console.warn("accept_proposal failed", e),
        ),
      ),
    );
  }

  async function rejectAllProposals() {
    if (!isTauri()) return;
    const commandTargets = realBatchProposals.slice();
    await Promise.all(
      commandTargets.map((proposal) =>
        invoke("reject_proposal", { callId: proposal.id }).catch((e) =>
          console.warn("reject_proposal failed", e),
        ),
      ),
    );
  }

  function reviseAllProposals() {
    void runEngineCommand(
      "Revise the current batch of proposals for stronger framing and cleaner transitions.",
    );
  }

  function toggleDeliveryTarget(key: string) {
    const next = deliveryTargetOverrides[key] === undefined
      ? !effectiveDeliveryTargets.some((target) => target.key === key && target.active)
      : !deliveryTargetOverrides[key];
    setDeliveryTargetOverrides((previous) => ({
      ...previous,
      [key]: next,
    }));
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
    invoke("accept_proposal", { callId: activeProposal.callId }).catch((e) =>
      console.warn("accept_proposal failed", e),
    );
  }

  function rejectActiveProposal() {
    if (!isTauri() || !activeProposal) return;
    invoke("reject_proposal", { callId: activeProposal.callId }).catch((e) =>
      console.warn("reject_proposal failed", e),
    );
  }

  function inspectActiveProposal() {
    setInspectorCollapsed(false);
    void runEngineCommand("Inspect the selected proposal in detail and list the supporting evidence.");
  }

  function reviseActiveProposal() {
    setInspectorCollapsed(false);
    void runEngineCommand("Revise the selected proposal and explain the tradeoffs.");
  }

  function openDeliveryFromChrome() {
    setStage(current ? "deliver" : "intent");
    setLens(current ? "delivery" : "import");
  }

  function openSettingsFromChrome() {
    setStage(current ? "indexing" : "intent");
    setLens(current ? "index" : "import");
    if (current) {
      void loadIndexerConfig();
    }
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
      }
    });
  });

  useEffect(() => {
    if (demoMode) {
      if (stage !== demoScreen.stage) {
        setStage(demoScreen.stage);
      }
      setLens(demoScreen.lens);
    }
  }, [demoMode, demoScreen.lens, demoScreen.stage, setLens, setStage, stage]);

  useEffect(() => {
    if (demoMode) {
      routedProjectRef.current = { project: null, mode: null };
      return;
    }
    if (current === null) {
      routedProjectRef.current = { project: null, mode: null };
      setStage("intent");
      setLens("import");
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
      setStage("proposal");
      setLens("review");
      return;
    }

    setStage("indexing");
    setLens(mode === "proxy" ? "index" : "import");
  }, [activeProposal, current, demoMode, hasImportedMedia, setLens, setStage, timelineDuration]);

  useEffect(() => {
    if (!demoMode && hasImportedMedia && currentLens === "import") {
      setLens("index");
    }
  }, [currentLens, demoMode, hasImportedMedia, setLens]);

  useEffect(() => {
    if (!demoMode) {
      void refreshMedia();
    }
  }, [current, demoMode, refreshMedia]);

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

  useEffect(() => {
    if (demoMode || !isTauri() || !selectedProxy) {
      setRealPreviewSrc(null);
      return;
    }
    let cancelled = false;
    mediaStreamUrl(selectedProxy.proxy_path)
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
  }, [demoMode, selectedProxy]);

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

  const activity: ActivityEntry[] = useMemo(() => {
    return items
      .filter(
        (it): it is ActivitySourceItem =>
          it.kind === "tool_call" || it.kind === "text" || it.kind === "job",
      )
      .slice(-12)
      .map((it) => activityFor(it));
  }, [items]);

  const activeJobs = useMemo(
    () =>
      items.filter(
        (it): it is Extract<typeof items[number], { kind: "job" }> =>
          it.kind === "job" && it.phase !== "completed",
      ),
    [items],
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
    return chips;
  }, [activeProposal, current, selectedStem, sourceMediaCount, timelineDuration, timelineSnapshot.cut_boundaries.length]);

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
    if (sources.length > 0) {
      return sources.map((source) => {
        const sourceStem = source.name.replace(/\.[^.]+$/, "");
        const proxy = proxies.find((entry) => entry.stem.startsWith(`${sourceStem}-`));
        return {
          id: source.id,
          title: source.name,
          detail: proxy
            ? `${formatBytes(source.size_bytes)} source · proxy ready`
            : `${formatBytes(source.size_bytes)} source · awaiting proxy/index`,
          status: transcodeJob ? "processing" : proxy ? "indexed" : importBusy ? "imported" : "partial",
          progress: transcodeJob?.percent ?? undefined,
        };
      });
    }
    return proxies.map((proxy) => ({
      id: proxy.stem,
      title: proxy.stem,
      detail: `${formatBytes(proxy.size_bytes)} proxy · ${proxy.proxy_path.split("/").pop() ?? "media"}`,
      status: transcodeJob ? "processing" : importBusy ? "imported" : "indexed",
      progress: transcodeJob?.percent ?? undefined,
    }));
  }, [activeJobs, proxies, sources]);

  const realIndexingTasks: IndexingTask[] = useMemo(() => {
    const taskJobs: Array<[IndexingTask["kind"], JobKind]> = [
      ["transcripts", "indexing"],
      ["scenes", "thumbnails"],
      ["audio", "waveform"],
      ["face", "indexing"],
      ["motion", "motion"],
      ["color", "indexing"],
      ["silence", "silences"],
      ["speaker", "indexing"],
      ["captions", "indexing"],
    ];
    return taskJobs.map(([kind, jobKind]) => {
      const runningJob = activeJobs.find((job) => job.job_kind === jobKind);
      const completed = completedJobKinds.has(jobKind);
      return {
        id: `real-${kind}`,
        kind,
        status: runningJob ? "indexing" : completed ? "indexed" : hasImportedMedia ? "missing" : "missing",
        progress: runningJob?.percent ?? (completed ? 100 : undefined),
        detail: runningJob?.status ?? (completed ? "Completed from local job state" : "Waiting for local indexer"),
      };
    });
  }, [activeJobs, completedJobKinds, hasImportedMedia]);

  const realIndexingReady = realIndexingTasks.some((task) => task.status === "indexed");
  const loadedTranscript =
    transcriptState?.state === "loaded" ? transcriptState.transcript : null;
  const realIndexingStructure: IndexingStructurePreview | undefined = useMemo(() => {
    if (sourceMediaCount === 0) return undefined;
    const duration = timelineDuration > 0
      ? timelineDuration
      : loadedTranscript?.segments.reduce((max, segment) => Math.max(max, segment.end_s), 0) ?? 0;
    const speakers = loadedTranscript
      ? loadedTranscript.speakers.length || new Set(loadedTranscript.segments.map((segment) => segment.speaker_id).filter(Boolean)).size
      : undefined;
    return {
      duration: duration > 0 ? formatDuration(duration) : undefined,
      scenes: timelineSnapshot.cut_boundaries.length || undefined,
      segments: loadedTranscript?.segments.length,
      speakers,
      transcriptPercent: loadedTranscript ? 100 : completedJobKinds.has("indexing") ? 100 : undefined,
    };
  }, [completedJobKinds, loadedTranscript, sourceMediaCount, timelineDuration, timelineSnapshot.cut_boundaries.length]);

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

  const effectiveDeliveryTargets: DeliveryTarget[] = useMemo(
    () =>
      realDeliveryTargets.map((target) => ({
        ...target,
        active: deliveryTargetOverrides[target.key] ?? target.active,
      })),
    [deliveryTargetOverrides, realDeliveryTargets],
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

  const realReviewSegments: ReviewTranscriptSegment[] = useMemo(
    () => (loadedTranscript ? transcriptToReviewSegments(loadedTranscript) : []),
    [loadedTranscript],
  );
  const realTranscriptCells: TranscriptCell[] = useMemo(
    () => (loadedTranscript ? transcriptToTimelineCells(loadedTranscript) : []),
    [loadedTranscript],
  );
  const realAgentEdits: AgentEdit[] = useMemo(
    () => timelineToAgentEdits(timelineSnapshot),
    [timelineSnapshot],
  );

  const realBatchProposals: BatchProposal[] = useMemo(() => {
    return items
      .filter(
        (it): it is Extract<typeof items[number], { kind: "proposed_edit" }> =>
          it.kind === "proposed_edit" && it.phase !== "completed",
      )
      .map((proposal, index) => ({
        id: proposal.id.toString(),
        title: proposal.summary || `Proposal ${index + 1}`,
        status: proposal.phase === "started" ? "processing" : "pending",
        timeRange: proposal.snapshot.duration_s > 0 ? formatDuration(proposal.snapshot.duration_s) : "Timeline proposal",
        cutType: proposal.source.source === "agent" ? "Agent proposal" : "User proposal",
        explanation: proposal.explanation ?? proposal.intent ?? proposal.summary,
        confidence: proposal.confidence,
        risk: mapProtocolRisk(proposal.risk),
      }));
  }, [items]);

  const realBatchCommands: AgentCommand[] = useMemo(() => {
    const userInputs = items.filter(
      (it): it is Extract<typeof items[number], { kind: "user_input" }> =>
        it.kind === "user_input",
    );
    if (userInputs.length === 0) {
      return activeProposal
        ? [
            {
              id: activeProposal.callId,
              text: activeProposal.summary,
              status: running ? "running" : "complete",
              proposalCount: realBatchProposals.length,
              startedAt: shortTime(),
            },
          ]
        : [];
    }
    return userInputs.slice(-8).map((input, index) => ({
      id: input.id.toString(),
      text: input.text,
      status: index === userInputs.length - 1 && running ? "running" : "complete",
      proposalCount: realBatchProposals.length || undefined,
      startedAt: shortTime(),
    }));
  }, [activeProposal, items, realBatchProposals.length, running]);

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

  // Stage routing — Proposal is the main working surface; Review,
  // Revise, Indexing, Deliver each take over the center pane with
  // their own surface. Intent still routes to the StageStub.
  const isProposalStage = stage === "proposal";
  const isReviewStage = stage === "review";
  const isReviseStage = stage === "revise";

  useEffect(() => {
    if (!isProposalStage) {
      setInspectorCollapsed(false);
    }
  }, [isProposalStage]);

  useEffect(() => {
    setInspectorCollapsed(false);
  }, [activeProposal?.callId]);

  const effectiveDuration = demoMode ? SCREEN2_DURATION_S : timelineDuration > 0 ? timelineDuration : sourceDurationS;
  const effectiveCurrentTime = demoMode ? SCREEN2_CURRENT_TIME_S : timelineDuration > 0 ? timelineTimeS : sourceCurrentTimeS;
  const effectiveChanges = demoMode ? screen2Changes : previewChanges;
  const effectivePlan = demoMode ? screen2Plan : plan;
  const effectiveInspector = demoMode ? screen2Inspector : inspectorData;
  const realIndexingWorkspace = (
    <IndexingDashboard
      projectName={current ? projectName(current) : undefined}
      sourceCount={sourceMediaCount}
      showImportActions={!hasImportedMedia}
      deliveryTarget={timelineDuration > 0 ? `Timeline ${formatDuration(timelineDuration)}` : undefined}
      media={realIndexingMedia}
      tasks={realIndexingTasks}
      structurePreview={realIndexingStructure}
      indexerConfig={indexerConfig}
      ready={realIndexingReady}
      onImport={() => void chooseAndImportFiles()}
      onImportUrl={() => setShowUrlImport(true)}
      onAskAgent={() => {
        setStage("proposal");
        setLens("review");
        void runEngineCommand("Create a first cut from the indexed media and explain the edit decisions.");
      }}
      onReviewIndexResults={() => {
        void loadIndexerConfig();
        void runIndexers();
      }}
      onToggleIndexer={(indexer) => void toggleProjectIndexer(indexer)}
      onOpenConfigPath={openConfigPath}
      onRevealConfigPath={revealConfigPath}
    />
  );
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
  const realWorkspace =
    !demoMode && !hasProject ? (
      <NoProjectWorkspace />
    ) : !demoMode && (currentLens === "import" || currentLens === "index") ? (
      realIndexingWorkspace
    ) : !demoMode && currentLens === "delivery" ? (
      realDeliveryWorkspace
    ) : !demoMode && isReviewStage && realBatchProposals.length > 0 ? (
      <BatchReviewSurface
        commands={realBatchCommands}
        proposals={realBatchProposals}
        selectedProposalId={activeProposal?.callId ?? realBatchProposals[0]?.id}
        insights={{
          pending: realBatchProposals.filter((proposal) => proposal.status === "pending" || proposal.status === "processing").length,
          accepted: realBatchProposals.filter((proposal) => proposal.status === "accepted").length,
          rejected: realBatchProposals.filter((proposal) => proposal.status === "rejected").length,
          avgConfidence: averageConfidence(realBatchProposals),
          riskLow: realBatchProposals.filter((proposal) => proposal.risk === "low").length,
          riskMedium: realBatchProposals.filter((proposal) => proposal.risk === "medium").length,
          riskHigh: realBatchProposals.filter((proposal) => proposal.risk === "high" || proposal.risk === "very-high").length,
        }}
        onAcceptOne={(proposal) => {
          if (!isTauri()) return;
          invoke("accept_proposal", { callId: proposal.id }).catch((e) =>
            console.warn("accept_proposal failed", e),
          );
        }}
        onRejectOne={(proposal) => {
          if (!isTauri()) return;
          invoke("reject_proposal", { callId: proposal.id }).catch((e) =>
            console.warn("reject_proposal failed", e),
          );
        }}
        onAcceptAll={() => void acceptAllProposals()}
        onRejectAll={() => void rejectAllProposals()}
        onReviseAll={reviseAllProposals}
        onPickCommand={(command) => {
          void runEngineCommand(`Continue this prior request: ${command.text}`);
        }}
        onInspectDeeper={(proposal) => {
          void runEngineCommand(`Inspect this proposal in detail and explain the evidence: ${proposal.title}`);
        }}
        onSendFeedback={(proposal) => {
          void runEngineCommand(`Revise this proposal using stricter pacing and delivery criteria: ${proposal.title}`);
        }}
      />
    ) : !demoMode && stage === "indexing" ? (
      realIndexingWorkspace
    ) : !demoMode && stage === "deliver" ? (
      realDeliveryWorkspace
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
        suggestions: demoMode ? screen2Suggestions : [],
        initialDraft: demoMode
          ? "Cut this into a tight 8-minute podcast episode.\nRemove dead air but preserve natural pacing."
          : undefined,
      };

  return (
    <>
    <AppShell
      topChromeStart={
        <Inline gap={demoMode ? "3" : "2"} align="center">
          {demoMode ? (
            <Inline gap="1" align="center" aria-hidden>
              <span className="h-3 w-3 rounded-full bg-[#ff5f57]" />
              <span className="h-3 w-3 rounded-full bg-[#febc2e]" />
              <span className="h-3 w-3 rounded-full bg-[#28c840]" />
            </Inline>
          ) : null}
          {demoMode ? (
            <span className="text-[13px] font-bold text-[var(--color-text-primary)]">Awidat</span>
          ) : (
            <img src={wordmark} alt="Awidat" className="h-6" />
          )}
        </Inline>
      }
      topChromeCenter={
        <Inline gap="3" align="center" className="min-w-0">
          <StageIndicator className="shrink-0" />
          {demoMode ? (
            <span className="min-w-0 truncate text-[var(--text-caption)] font-semibold text-[var(--color-text-secondary)]">
              {demoScreen.specLabel} · {demoScreen.title}
            </span>
          ) : null}
        </Inline>
      }
      topChromeEnd={
          <Inline gap="1" align="center">
          {activeProposal || demoMode ? (
            <Pill status="warning">{demoMode ? demoScreen.pendingLabel ?? "Demo" : `${effectiveChanges.length} pending`}</Pill>
          ) : null}
          <AgentStatusBadge
            status={
              demoMode
                ? "awaiting-review"
                : running
                ? "analyzing"
                : activeProposal
                  ? "awaiting-review"
                  : current
                    ? "online"
                    : "idle"
            }
            detail={demoMode ? demoScreen.statusLabel : running ? "Working" : activeProposal ? "Review pending" : current ? "Ready" : "No project"}
          />
          {demoMode ? (
            <>
              <IconButton icon={<Undo2 />} label="Undo" size="md" />
              <IconButton icon={<Redo2 />} label="Redo" size="md" />
              <IconButton icon={<CircleHelp />} label="Help" size="md" />
              <IconButton icon={<Bell />} label="Notifications" size="md" />
              <span className="grid h-5 w-5 place-items-center rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[9px] font-semibold text-[var(--color-text-secondary)]">
                T
              </span>
            </>
          ) : null}
          <IconButton icon={<Share2 />} label="Share" size="md" onClick={openDeliveryFromChrome} />
          <IconButton icon={<SettingsIcon />} label="Settings" size="md" onClick={openSettingsFromChrome} />
        </Inline>
      }
      lensRow={<LensNav showImport={!hasImportedMedia} />}
      workspace={demoMode && demoScreen.workspace ? demoScreen.workspace : realWorkspace}
      commandRail={
        <CommandRail
          hasProject={hasProject || demoMode}
          running={demoMode ? true : running}
          {...railProps}
          onSubmit={(command) => void runEngineCommand(command)}
          onCancel={() => {
            if (!isTauri()) return;
            invoke("cancel_turn").catch((e) =>
              console.warn("cancel_turn failed", e),
            );
          }}
          onSuggestion={(action) => void runEngineCommand(action.prompt)}
          onRemoveChip={(chip) => dismissContextChip(chip)}
        />
      }
      preview={
        isProposalStage ? (
          <PreviewSurface
            proposalName={demoMode ? "Podcast Tightening v1" : activeProposal?.summary ?? "Source review"}
            pendingCount={effectiveChanges.length}
            changes={effectiveChanges}
            activeChangeId={demoMode ? "c07" : undefined}
            currentTimeS={effectiveCurrentTime}
            durationS={effectiveDuration}
            isPlaying={demoMode ? false : isPlaying}
            videoSlot={
              demoMode ? (
                <Screen2MediaSlot />
              ) : realPreviewSrc && selectedProxy ? (
                <RealProxyPreviewSlot
                  src={realPreviewSrc}
                  stem={selectedProxy.stem}
                  isPlaying={isPlaying}
                  seekRequestId={sourceSeekRequestId}
                  seekTargetS={sourceSeekTargetS}
                  posterSrc={realVideoFrames[0]}
                  onTime={setSourceTime}
                  onDuration={setSourceDuration}
                  onPlaying={setMediaPlaying}
                />
              ) : undefined
            }
            onPlayPause={() => setMediaPlaying(!isPlaying)}
            onSeek={requestSourceSeek}
            onOpenProposalMenu={() => setInspectorCollapsed(false)}
            onInspectProposal={inspectActiveProposal}
            onReviseProposal={reviseActiveProposal}
            onAcceptProposal={activeProposal ? acceptActiveProposal : undefined}
            onRejectProposal={activeProposal ? rejectActiveProposal : undefined}
            onFullscreen={() => {
              setStage("review");
              setLens("review");
            }}
          />
        ) : isReviewStage ? (
          <ReviewLensSurface
            segments={realReviewSegments}
            durationS={effectiveDuration}
            currentTimeS={effectiveCurrentTime}
          />
        ) : isReviseStage ? (
          <ReviseSurface
            suggestions={[
              "Tighten the open by lifting the host's welcome.",
              "Make this section slower.",
              "Use fewer transitions.",
            ]}
          />
        ) : stage === "indexing" ? (
          realIndexingWorkspace
        ) : stage === "deliver" ? (
          realDeliveryWorkspace
        ) : (
          <StageStub stage={stage} onPrimaryAction={() => setStage("proposal")} />
        )
      }
      timeline={
        isProposalStage ? (
          <TimelineHybrid
            tab={timelineTab}
            onChangeTab={setTimelineTab}
            viewMode={timelineViewMode}
            onChangeViewMode={setTimelineViewMode}
            durationS={effectiveDuration}
            currentTimeS={effectiveCurrentTime}
            changeCount={effectiveChanges.length}
            videoFrames={demoMode ? screen2Frames : realVideoFrames}
            audioPeaks={demoMode ? screen2AudioPeaks : realAudioPeaks}
            agentEdits={demoMode ? screen2AgentEdits : realAgentEdits}
            transcript={demoMode ? screen2Transcript : realTranscriptCells}
            diff={demoMode ? screen2Diff : []}
          />
        ) : (
          <span />
        )
      }
      inspector={
        isProposalStage && inspectorCollapsed ? (
          <CollapsedInspectorButton onOpen={() => setInspectorCollapsed(false)} />
        ) : isProposalStage ? (
          <ProposalInspector
            data={effectiveInspector}
            onAccept={acceptActiveProposal}
            onReject={rejectActiveProposal}
            onInspectDeeper={inspectActiveProposal}
            onRevise={reviseActiveProposal}
            onAgentRepair={() => {
              void runEngineCommand("Repair the selected proposal's risky edits before acceptance.");
            }}
            onMaximize={() => {
              setStage("review");
              setLens("review");
            }}
            onCollapse={() => setInspectorCollapsed(true)}
          />
        ) : (
          <span />
        )
      }
      inspectorCollapsed={isProposalStage && inspectorCollapsed}
      footer={<Footer demoMode={demoMode} />}
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

function RealProxyPreviewSlot({
  src,
  stem,
  isPlaying,
  seekRequestId,
  seekTargetS,
  posterSrc,
  onTime,
  onDuration,
  onPlaying,
}: {
  src: string;
  stem: string;
  isPlaying: boolean;
  seekRequestId: number;
  seekTargetS: number;
  posterSrc?: string;
  onTime: (timeS: number) => void;
  onDuration: (durationS: number) => void;
  onPlaying: (playing: boolean) => void;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [hasPaintedFrame, setHasPaintedFrame] = useState(false);

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
        onLoadedData={() => setHasPaintedFrame(true)}
        onCanPlay={() => setHasPaintedFrame(true)}
        onTimeUpdate={(event) => {
          setHasPaintedFrame(true);
          onTime(event.currentTarget.currentTime);
        }}
        onPlay={() => onPlaying(true)}
        onPause={() => onPlaying(false)}
        onEnded={() => onPlaying(false)}
        onClick={() => onPlaying(!isPlaying)}
      />
      <div className="pointer-events-none absolute left-3 top-3 rounded-[var(--radius-sm)] border border-black/50 bg-black/70 px-2 py-1">
        <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-white/70">
          Source proxy
        </span>
        <span className="ml-2 font-mono text-[var(--text-caption)] text-white/90">{stem}</span>
      </div>
    </div>
  );
}

function CollapsedInspectorButton({ onOpen }: { onOpen: () => void }) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className="flex h-full w-full flex-col items-center justify-start gap-2 px-1 py-3 text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]"
      aria-label="Open proposal inspector"
      title="Open proposal inspector"
    >
      <PanelRightOpen className="h-4 w-4 shrink-0 stroke-[1.75]" />
      <span
        className="font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--text-caption)]"
        style={{ writingMode: "vertical-rl" }}
      >
        Inspector
      </span>
    </button>
  );
}

function NoProjectWorkspace() {
  return (
    <div className="flex h-full min-h-0 items-center justify-center bg-[var(--color-surface-app)] p-4">
      <div className="grid w-full max-w-4xl grid-cols-[minmax(0,1fr)_280px] gap-3">
        <Card padding="lg" tone="elevated" className="min-h-[340px]">
          <Stack gap="3" align="center" className="h-full justify-center text-center">
            <span className="relative flex h-16 w-16 items-center justify-center rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-brand-secondary)]">
              <Film className="h-7 w-7 stroke-[1.5]" />
              <span className="absolute -right-1.5 -top-1.5 flex h-6 w-6 items-center justify-center rounded-full border border-[var(--color-border-active)] bg-[var(--color-surface-selected)]">
                <ImportIcon className="h-3.5 w-3.5 stroke-[1.75]" />
              </span>
            </span>
            <Stack gap="2" align="center" className="max-w-md">
              <span className="text-[var(--text-h1)] font-semibold text-[var(--color-text-primary)]">
                No project open yet
              </span>
              <span className="text-[var(--text-body)] leading-relaxed text-[var(--color-text-secondary)]">
                Import media or create a new project to get started with intelligent proposals and review.
              </span>
            </Stack>
            <Inline gap="2" wrap="wrap" justify="center">
              <Button
                variant="primary"
                leadingIcon={<ImportIcon className="h-3.5 w-3.5 stroke-[1.75]" />}
                onClick={() => emitMenuCommand(MENU_COMMANDS.IMPORT_FILES)}
              >
                Import media
              </Button>
              <Button
                variant="secondary"
                leadingIcon={<FolderOpen className="h-3.5 w-3.5 stroke-[1.75]" />}
                onClick={() => emitMenuCommand(MENU_COMMANDS.OPEN_PROJECT)}
              >
                Open project
              </Button>
              <Button
                variant="ghost"
                leadingIcon={<Play className="h-3.5 w-3.5 stroke-[1.75]" />}
                onClick={() => emitMenuCommand(MENU_COMMANDS.NEW_PROJECT)}
              >
                Start with example
              </Button>
            </Inline>
            <Card padding="sm" tone="flat" className="max-w-md text-left">
              <span className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-muted)]">
                Awidat's agents will analyze your content and propose a tight, publish-ready edit.
              </span>
            </Card>
          </Stack>
        </Card>
        <div className="grid gap-3">
          <Card padding="md">
            <Stack gap="3">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                What happens next
              </span>
              {[
                "Media is indexed locally first.",
                "The agent builds proposals with evidence.",
                "You review, revise, and deliver safely.",
              ].map((item, index) => (
                <Inline key={item} gap="2" align="center">
                  <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] font-mono text-[var(--text-caption)] text-[var(--color-brand-secondary)]">
                    {index + 1}
                  </span>
                  <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
                    {item}
                  </span>
                </Inline>
              ))}
            </Stack>
          </Card>
          <Card padding="md">
            <Stack gap="2">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                System state
              </span>
              <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
                Ready for local project
              </span>
              <span className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
                No media is loaded, no proposal is active, and no timeline changes can be applied until a project is opened.
              </span>
            </Stack>
          </Card>
        </div>
      </div>
    </div>
  );
}

function Footer({ demoMode = false }: { demoMode?: boolean }) {
  const running = useAgentStore((s) => s.running);
  const items = useAgentStore((s) => s.items);
  if (demoMode) {
    return (
      <>
        <Inline gap="3" align="center">
          <span className="h-2 w-2 rounded-full bg-[var(--color-success)]" aria-hidden />
          <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)] font-mono">
            Agent online
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
            Model: Awidat Pro 1.2
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
            Context window: 42m
          </span>
        </Inline>
        <Inline gap="3" align="center">
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
            Autosaved 12:42:18 ✓
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
            Render queue 1
          </span>
          <span className="inline-flex h-3.5 items-end gap-0.5" aria-hidden>
            {[5, 9, 4, 11, 7].map((h, i) => (
              <span
                key={i}
                className="w-1 rounded-full bg-[var(--color-brand-secondary)]/60"
                style={{ height: h }}
              />
            ))}
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
            Disk 1.2 TB free
          </span>
        </Inline>
      </>
    );
  }
  const activeJobs = items.filter((item) => item.kind === "job" && item.phase !== "completed");
  const renderQueueLabel = activeJobs.length > 0 ? activeJobs.length.toString() : "0";
  return (
    <>
      <Inline gap="3" align="center" className="min-w-0">
        <span
          className="h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: running ? "var(--color-warning)" : "var(--color-success)" }}
          aria-hidden
        />
        <span className="shrink-0 text-[var(--text-caption)] text-[var(--color-text-secondary)] font-mono">
          {running ? "Agent working" : "Agent online"}
        </span>
        <span className="shrink-0 text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          Model: Awidat Pro 1.2
        </span>
        <span className="min-w-0 truncate text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          Context window: local
        </span>
      </Inline>
      <Inline gap="3" align="center" className="min-w-0 justify-end">
        <span className="shrink-0 text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          Autosaved local ✓
        </span>
        <span className="shrink-0 text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          Render queue {renderQueueLabel}
        </span>
        <span className="inline-flex h-3.5 shrink-0 items-end gap-0.5" aria-hidden>
          {[4, 8, 5, 10, 6].map((h, i) => (
            <span
              key={i}
              className="w-1 rounded-full bg-[var(--color-brand-secondary)]/60"
              style={{ height: h }}
            />
          ))}
        </span>
        <span className="shrink-0 text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          Disk local
        </span>
      </Inline>
    </>
  );
}

function projectName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
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

function jobKindLabel(kind: string): string {
  return kind
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function mapProtocolRisk(risk: "low" | "medium" | "high" | "very_high" | undefined) {
  if (!risk) return undefined;
  return risk === "very_high" ? "very-high" : risk;
}

function averageConfidence(proposals: BatchProposal[]): number | undefined {
  const values = proposals
    .map((proposal) => proposal.confidence)
    .filter((value): value is number => typeof value === "number");
  if (values.length === 0) return undefined;
  return values.reduce((sum, value) => sum + value, 0) / values.length;
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

function transcriptToReviewSegments(transcript: Transcript): ReviewTranscriptSegment[] {
  return transcript.segments.slice(0, 120).map((segment, index) => {
    const confidence = confidenceForSegment(segment.text);
    return {
      id: `${transcript.asset_stem}-${index}`,
      speaker: speakerLabel(segment.speaker_id, index),
      speakerColor: speakerColor(segment.speaker_id, index),
      startTime: formatClock(segment.start_s),
      startS: segment.start_s,
      endS: segment.end_s,
      text: segment.text,
      confidence,
      state: confidence < 0.55 ? "warning" : "default",
      evidence: evidenceForTranscriptSegment(segment.text),
    };
  });
}

function transcriptToTimelineCells(transcript: Transcript): TranscriptCell[] {
  return transcript.segments.slice(0, 80).map((segment, index) => ({
    id: `${transcript.asset_stem}-tl-${index}`,
    startS: segment.start_s,
    endS: segment.end_s,
    text: segment.text,
    speakerColor: speakerColor(segment.speaker_id, index),
  }));
}

function timelineToAgentEdits(snapshot: TimelineSnapshot): AgentEdit[] {
  const edits: AgentEdit[] = [];
  for (const boundary of snapshot.cut_boundaries) {
    const at = findClipStart(snapshot, boundary.to_clip_id);
    if (at === null) continue;
    edits.push({
      id: boundary.key,
      startS: Math.max(0, at - 0.25),
      endS: at + 0.75,
      status:
        boundary.confidence !== null && boundary.confidence < 0.55
          ? "warning"
          : "reviewing",
      label: `${boundary.cut_type} · ${boundary.intent}`,
    });
  }
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.audio_lead_s || item.audio_trail_s || item.split_edit_reason) {
        edits.push({
          id: `${track.name}-${item.index}-split`,
          startS: item.track_start_s,
          endS: item.track_start_s + Math.max(0.6, Math.min(4, item.duration_s)),
          status: item.split_edit_confidence !== null && item.split_edit_confidence < 0.6 ? "warning" : "reviewing",
          label: item.split_edit_reason ?? "Split edit",
        });
      }
    }
  }
  return edits.slice(0, 80);
}

function findClipStart(snapshot: TimelineSnapshot, clipId: string): number | null {
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind === "clip" && item.clip_uuid === clipId) {
        return item.track_start_s;
      }
    }
  }
  return null;
}

function speakerLabel(speakerId: string | null, index: number): string {
  if (!speakerId) return index % 2 === 0 ? "A" : "B";
  const numeric = speakerId.match(/\d+$/)?.[0];
  if (numeric) return String.fromCharCode(65 + (Number.parseInt(numeric, 10) % 26));
  return speakerId.slice(0, 1).toUpperCase();
}

function speakerColor(speakerId: string | null, index: number): string {
  const n = speakerId?.match(/\d+$/)?.[0];
  const speakerIndex = n ? Number.parseInt(n, 10) : index;
  return speakerIndex % 2 === 0 ? "var(--color-viz-speaker-a)" : "var(--color-viz-speaker-b)";
}

function confidenceForSegment(text: string): number {
  const lower = text.toLowerCase();
  const fillerHits = [" um ", " uh ", " basically ", " kind of ", " sort of "].filter((token) =>
    ` ${lower} `.includes(token),
  ).length;
  return Math.max(0.42, 0.92 - fillerHits * 0.14);
}

function evidenceForTranscriptSegment(text: string): { id: string; label: string }[] {
  const lower = ` ${text.toLowerCase()} `;
  const evidence: { id: string; label: string }[] = [];
  if ([" um ", " uh ", " basically ", " kind of ", " sort of "].some((token) => lower.includes(token))) {
    evidence.push({ id: "filler", label: "Filler language" });
  }
  if (text.length > 140) {
    evidence.push({ id: "long", label: "Long sentence" });
  }
  if (evidence.length === 0) {
    evidence.push({ id: "transcript", label: "Transcript boundary" });
  }
  return evidence;
}

function formatClock(totalSeconds: number): string {
  const safe = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const seconds = safe % 60;
  return `${hours.toString().padStart(2, "0")}:${minutes
    .toString()
    .padStart(2, "0")}:${seconds.toString().padStart(2, "0")}`;
}

type AnyAgentItem = ReturnType<typeof useAgentStore.getState>["items"][number];
type ToolCallItem = Extract<AnyAgentItem, { kind: "tool_call" }>;
type TextItem = Extract<AnyAgentItem, { kind: "text" }>;
type JobItem = Extract<AnyAgentItem, { kind: "job" }>;
type ActivitySourceItem = ToolCallItem | TextItem | JobItem;

function activityFor(item: ActivitySourceItem): ActivityEntry {
  const id = item.id.toString();
  if (item.kind === "tool_call") {
    return {
      id,
      timestamp: shortTime(),
      text: `${item.name} ${item.phase === "completed" ? "complete" : "running"}`,
      kind: "tool",
    };
  }
  if (item.kind === "text") {
    return {
      id,
      timestamp: shortTime(),
      text: item.text.slice(0, 80),
      kind: "thought",
    };
  }
  return {
    id,
    timestamp: shortTime(),
    text: `${item.job_kind} · ${item.status}`,
    kind: "result",
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
