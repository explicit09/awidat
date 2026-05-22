import {
  AudioWaveform,
  Bot,
  Captions,
  CircleAlert,
  CircleCheck,
  Clock,
  Database,
  Eye,
  FileText,
  Film,
  FolderOpen,
  GitBranch,
  Import,
  Layers3,
  LayoutDashboard,
  Loader,
  MessageSquareText,
  PackageCheck,
  PanelRight,
  Play,
  RotateCw,
  SearchCheck,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Terminal,
  Users,
  VolumeX,
  type LucideIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import type { Stage } from "../state/stages";
import type { Lens } from "../state/lenses";
import {
  AgentStatusBadge,
  Button,
  Card,
  ConfidenceMeter,
  ConfidenceRing,
  EvidenceChip,
  Inline,
  MediaStatusRow,
  Pill,
  ReviewActions,
  RiskIndicator,
  Stack,
  TimelineMarker,
  cn,
  type ChannelLane,
  type PillStatus,
} from "../ui";
import type { CommandRailProps } from "./CommandRail";
import { BatchReviewSurface, type AgentCommand, type BatchProposal } from "./BatchReviewSurface";
import { DeliverySurface, type DeliveryRenderSummary, type DeliveryTarget, type PreflightFinding } from "./DeliverySurface";
import { IndexingDashboard, type IndexingMediaItem, type IndexingTask } from "./IndexingDashboard";
import { ProposalInspector, type ProposalInspectorData } from "./ProposalInspector";
import type { ReviewTranscriptSegment } from "./ReviewLensSurface";
import podcastAfter from "./assets/podcast-after.jpg";
import podcastBefore from "./assets/podcast-before.jpg";
import podcastThumb01 from "./assets/podcast-thumb-01.jpg";
import podcastThumb02 from "./assets/podcast-thumb-02.jpg";
import podcastThumb03 from "./assets/podcast-thumb-03.jpg";
import podcastThumb04 from "./assets/podcast-thumb-04.jpg";
import podcastThumb05 from "./assets/podcast-thumb-05.jpg";
import podcastWide from "./assets/podcast-wide.jpg";

export const DEMO_SCREEN_IDS = ["screen1", "screen2", "screen3", "screen4", "screen5", "screen6", "screen7", "screen8", "screen9"] as const;
export type DemoScreenId = (typeof DEMO_SCREEN_IDS)[number];

export type DemoScreenDefinition = {
  id: DemoScreenId;
  specLabel: string;
  title: string;
  stage: Stage;
  lens: Lens;
  statusLabel: string;
  pendingLabel?: string;
  commandRail?: CommandRailProps;
  workspace?: ReactNode;
  standalone?: boolean;
  smokeText: string[];
};

export function resolveDemoScreenId(search: string): DemoScreenId {
  const params = new URLSearchParams(search);
  const requested = params.get("awidatScreen") ?? params.get("screen");
  return DEMO_SCREEN_IDS.includes(requested as DemoScreenId)
    ? (requested as DemoScreenId)
    : "screen2";
}

const batchCommands: AgentCommand[] = [
  {
    id: "cmd-1",
    text: "Generate 5 short clips from the strongest moments.",
    status: "complete",
    proposalCount: 5,
    startedAt: "12:31",
  },
  {
    id: "cmd-2",
    text: "Tighten hooks and package for YouTube Shorts.",
    status: "running",
    proposalCount: 3,
    startedAt: "12:38",
  },
  {
    id: "cmd-3",
    text: "Review captions and safe-area framing.",
    status: "queued",
    proposalCount: 2,
    startedAt: "12:41",
  },
];

const batchProposals: BatchProposal[] = [
  {
    id: "short-1",
    title: "The #1 thing that kills startups",
    status: "pending",
    timeRange: "00:06:35:21 – 00:07:04:18",
    cutType: "Hook + punchline",
    explanation: "Strong branded phrase, clean handoff, and a natural title-card moment.",
    confidence: 0.92,
    risk: "low",
    thumbnail: podcastThumb01,
  },
  {
    id: "short-2",
    title: "Biggest mistake most founders make",
    status: "pending",
    timeRange: "00:08:12:03 – 00:08:39:20",
    cutType: "Jump-cut sequence",
    explanation: "Removes filler while keeping the guest's thought intact.",
    confidence: 0.89,
    risk: "medium",
    thumbnail: podcastThumb02,
  },
  {
    id: "short-3",
    title: "Product vs. Market: The truth",
    status: "pending",
    timeRange: "00:14:02:10 – 00:14:37:02",
    cutType: "Story clip",
    explanation: "Best standalone moment for vertical distribution.",
    confidence: 0.85,
    risk: "low",
    thumbnail: podcastThumb03,
  },
  {
    id: "short-4",
    title: "How to get your first 100 users",
    status: "accepted",
    timeRange: "00:18:40:04 – 00:19:22:10",
    cutType: "Founder lesson",
    explanation: "Clear tactical advice with a crisp end beat and caption-safe framing.",
    confidence: 0.78,
    risk: "low",
    thumbnail: podcastThumb04,
  },
  {
    id: "short-5",
    title: "Why growth does not fix broken product",
    status: "rejected",
    timeRange: "00:23:02:11 – 00:24:21:08",
    cutType: "Long-form extract",
    explanation: "Useful idea, but the proposed cut runs long for TikTok and needs a sharper hook.",
    confidence: 0.72,
    risk: "medium",
    thumbnail: podcastThumb05,
  },
];

const reviewSegments: ReviewTranscriptSegment[] = [
  {
    id: "seg-1",
    speaker: "J",
    speakerColor: "var(--color-viz-speaker-a)",
    startTime: "03:36:10",
    startS: 216.4,
    endS: 221.8,
    text: "So, the short answer is yes, but the long answer is a bit more nuanced.",
    confidence: 0.91,
    state: "accepted",
    evidence: [{ id: "ev-1", label: "Clean phrase" }],
  },
  {
    id: "seg-2",
    speaker: "J",
    speakerColor: "var(--color-viz-speaker-a)",
    startTime: "03:41:12",
    startS: 221.9,
    endS: 226.8,
    text: "Um, so basically what we found was...",
    confidence: 0.86,
    state: "selected",
    evidence: [
      { id: "ev-2", label: "Filler (um, so)" },
      { id: "ev-3", label: "Pause 0.9s" },
    ],
  },
  {
    id: "seg-3",
    speaker: "A",
    speakerColor: "var(--color-viz-speaker-b)",
    startTime: "03:46:08",
    startS: 226.9,
    endS: 229.8,
    text: "That changed everything.",
    confidence: 0.93,
    state: "accepted",
    evidence: [{ id: "ev-4", label: "Speaker handoff" }],
  },
  {
    id: "seg-4",
    speaker: "J",
    speakerColor: "var(--color-viz-speaker-a)",
    startTime: "03:49:02",
    startS: 229.9,
    endS: 234.7,
    text: "Yeah exactly, and I think that's a really good point.",
    confidence: 0.68,
    state: "warning",
    evidence: [{ id: "ev-5", label: "Cadence dip" }],
  },
  {
    id: "seg-5",
    speaker: "A",
    speakerColor: "var(--color-viz-speaker-b)",
    startTime: "03:53:14",
    startS: 234.8,
    endS: 240.4,
    text: "Right, because users are paying attention to the thing that happens next.",
    confidence: 0.74,
    state: "default",
    evidence: [{ id: "ev-6", label: "Pending review" }],
  },
];

const semanticLanes: ChannelLane[] = [
  {
    id: "wave",
    label: "Waveform",
    paletteKey: "energy",
    segments: [
      { startS: 216, endS: 221.8, value: 0.62 },
      { startS: 221.9, endS: 226.8, value: 0.22 },
      { startS: 226.9, endS: 229.8, value: 0.76 },
      { startS: 229.9, endS: 234.7, value: 0.48 },
      { startS: 234.8, endS: 240.4, value: 0.57 },
    ],
  },
  {
    id: "speakers",
    label: "Speakers",
    paletteKey: "speaker",
    segments: [
      { startS: 216, endS: 226.8, key: "0" },
      { startS: 226.9, endS: 229.8, key: "1" },
      { startS: 229.9, endS: 234.7, key: "0" },
      { startS: 234.8, endS: 240.4, key: "1" },
    ],
  },
  {
    id: "changes",
    label: "Changes",
    segments: [
      { startS: 216, endS: 221.8, color: "rgba(34, 197, 94, 0.82)" },
      { startS: 221.9, endS: 226.8, color: "rgba(245, 158, 11, 0.86)" },
      { startS: 226.9, endS: 229.8, color: "rgba(34, 197, 94, 0.82)" },
      { startS: 229.9, endS: 234.7, color: "rgba(234, 179, 8, 0.78)" },
      { startS: 234.8, endS: 240.4, color: "rgba(245, 158, 11, 0.72)" },
    ],
  },
  {
    id: "confidence",
    label: "Confidence",
    paletteKey: "confidence",
    segments: [
      { startS: 216, endS: 221.8, value: 0.91 },
      { startS: 221.9, endS: 226.8, value: 0.86 },
      { startS: 226.9, endS: 229.8, value: 0.93 },
      { startS: 229.9, endS: 234.7, value: 0.68 },
      { startS: 234.8, endS: 240.4, value: 0.74 },
    ],
  },
];

const cutInspectorData: ProposalInspectorData = {
  title: "Cut 12 · L-cut",
  status: "pending",
  statusLabel: "Pending",
  timeRange: "00:06:42:11 – 00:06:45:02",
  duration: "2.91s",
  cutType: "L-cut (audio leads video)",
  intent:
    "Remove excess pause after the question to preserve pacing and smooth the speaker handoff. Let the response audio lead into the next shot for flow.",
  confidence: 0.92,
  risk: "low",
  evidence: [
    { id: "ci-1", kind: "transcript", label: "Transcript boundary", confidenceLevel: "high" },
    { id: "ci-2", kind: "cut_boundary", label: "Scene change", confidenceLevel: "high" },
    { id: "ci-3", kind: "audio_energy", label: "Audio energy", confidenceLevel: "high" },
    { id: "ci-4", kind: "speaker_handoff", label: "Speaker change", confidenceLevel: "high" },
    { id: "ci-5", kind: "pacing", label: "Beat / rhythm", confidenceLevel: "medium" },
    { id: "ci-6", kind: "visual", label: "Visual continuity", confidenceLevel: "high" },
  ],
  explanation:
    "The guest finishes their question and there's a 0.9s pause before the host begins. The host's response starts with immediate energy and on-beat cadence. An L-cut preserves momentum and creates a natural handoff.",
  alternatives: [
    { id: "alt-1", label: "Keep full pause", detail: "+0.9s" },
    { id: "alt-2", label: "Tighter cut", detail: "-0.4s" },
    { id: "alt-3", label: "Swap cutaway", detail: "B-cam" },
  ],
  alternativesCountLabel: "Compare alternatives",
};

const indexingMedia: IndexingMediaItem[] = [
  {
    id: "m1",
    title: "A_CAM_Main.mp4",
    detail: "4.2 GB · 00:42:11 · 4K",
    status: "indexing",
    progress: 67,
    thumbnail: podcastThumb01,
  },
  {
    id: "m2",
    title: "B_CAM_Guest.mp4",
    detail: "3.9 GB · 00:42:10 · 4K",
    status: "indexed",
    progress: 100,
    thumbnail: podcastThumb02,
  },
  {
    id: "m3",
    title: "Podcast_Master.wav",
    detail: "1.1 GB · 48 kHz · 24-bit",
    status: "processing",
    progress: 42,
    thumbnail: podcastThumb04,
  },
  {
    id: "m4",
    title: "Wide_Shot.mp4",
    detail: "1.8 GB · 00:42:11 · 4K",
    status: "indexed",
    progress: 100,
    thumbnail: podcastWide,
  },
  {
    id: "m5",
    title: "Intro_Music.mp3",
    detail: "24.1 MB · 00:01:30",
    status: "indexed",
    progress: 100,
  },
  {
    id: "m6",
    title: "Logo.png",
    detail: "2.1 MB · 1920x1080",
    status: "indexed",
    progress: 100,
  },
  {
    id: "m7",
    title: "Notes.txt",
    detail: "3.2 KB",
    status: "indexed",
    progress: 100,
  },
  {
    id: "m8",
    title: "Questions.xlsx",
    detail: "18.7 KB",
    status: "missing",
  },
  {
    id: "m9",
    title: "Rough_Style_Ref.mov",
    detail: "512 MB · 00:00:30 · 1080p",
    status: "imported",
  },
];

const indexingTasks: IndexingTask[] = [
  { id: "idx-1", kind: "transcripts", status: "indexed", progress: 100, detail: "42 minutes transcribed" },
  { id: "idx-2", kind: "scenes", status: "indexed", progress: 100, detail: "31 scene boundaries" },
  { id: "idx-3", kind: "audio", status: "indexing", progress: 74, detail: "Energy, silence, and clipping" },
  { id: "idx-4", kind: "face", status: "indexing", progress: 58, detail: "Speaker presence and framing" },
  { id: "idx-5", kind: "motion", status: "partial", progress: 36, detail: "Camera movement and stability" },
  { id: "idx-6", kind: "color", status: "processing", progress: 44, detail: "Exposure and continuity" },
  { id: "idx-7", kind: "silence", status: "indexed", progress: 100, detail: "Dead air candidates found" },
  { id: "idx-8", kind: "speaker", status: "indexing", progress: 64, detail: "Host / guest turns" },
  { id: "idx-9", kind: "captions", status: "partial", progress: 28, detail: "Caption-safe phrases" },
];

const deliveryTargets: DeliveryTarget[] = [
  { key: "youtube", active: true, label: "YouTube 16:9", spec: "1920x1080" },
  { key: "tiktok", active: true, label: "TikTok 9:16", spec: "1080x1920" },
  { key: "instagram", active: false, label: "Podcast full", spec: "1920x1080" },
  { key: "captions", active: true, label: "Captions burn-in", spec: "1920x1080" },
  { key: "cover", active: true, label: "Clean master", spec: "1920x1080" },
  { key: "custom", active: false, label: "Custom preset", spec: "User selected" },
];

const preflightFindings: PreflightFinding[] = [
  {
    id: "pf-1",
    severity: "pass",
    message: "Loudness (Integrated)",
    asset: "-16.2 LUFS",
  },
  {
    id: "pf-2",
    severity: "pass",
    message: "Missing media",
    asset: "All media found",
  },
  {
    id: "pf-3",
    severity: "warning",
    time: "00:06:42:05",
    message: "Caption safe area",
    asset: "Short 01 · TikTok 9:16",
    suggestedFix: "Shift lower-third 36 px upward.",
  },
  {
    id: "pf-4",
    severity: "pass",
    message: "Aspect ratio",
    asset: "Timeline 16:9 matches target 16:9",
  },
  {
    id: "pf-5",
    severity: "pass",
    message: "Codec",
    asset: "Podcast_Tightening_v1.mp4",
  },
  {
    id: "pf-6",
    severity: "pass",
    message: "Frame rate",
    asset: "29.97 fps matches target",
  },
  {
    id: "pf-7",
    severity: "warning",
    time: "00:08:12:03",
    message: "Render queue risks",
    asset: "2 high-complexity segments",
    suggestedFix: "Pre-render transitions before export.",
  },
  {
    id: "pf-8",
    severity: "pass",
    message: "Title safe area",
    asset: "All important text within title safe",
  },
  {
    id: "pf-9",
    severity: "pass",
    message: "Estimated render size",
    asset: "~1.36 GB",
  },
  {
    id: "pf-10",
    severity: "failure",
    message: "Estimated render time",
    asset: "00:38:42 exceeds target max 00:30:00",
    suggestedFix: "Use agent repair to pre-render high-complexity segments.",
  },
];

const deliverySummary: DeliveryRenderSummary = {
  duration: "00:28:31:12",
  estimatedSize: "~1.36 GB",
  outputs: 4,
  confidence: 0.82,
};

const screen3CommandRail: CommandRailProps = {
  hasProject: true,
  running: true,
  initialDraft: "Create a short clips pack from the best podcast moments.\nKeep each clip self-contained and safe for vertical delivery.",
  contextChips: [
    { label: "Clip pack: Shorts v2", kind: "project" },
    { label: "Range: full episode", kind: "media" },
    { label: "Target: Shorts 9:16", kind: "lens" },
  ],
  taskProgress: { label: "Packaging proposal set...", progress: 76, eta: "00:00:52" },
  plan: [
    { id: "p1", text: "Rank candidate hooks", status: "complete" },
    { id: "p2", text: "Build clip boundaries", status: "complete" },
    { id: "p3", text: "Review captions and safe areas", status: "in_progress" },
    { id: "p4", text: "Prepare delivery package", status: "pending" },
  ],
  activity: [
    { id: "a1", timestamp: "12:37", text: "Found 14 candidate short-form moments" },
    { id: "a2", timestamp: "12:39", text: "Generated 5 clip proposals" },
  ],
  suggestions: [
    { id: "s1", label: "Revise hooks", prompt: "Make the opening lines sharper" },
    { id: "s2", label: "Package clips", prompt: "Send accepted clips to preflight" },
  ],
};

export const demoScreens: Record<DemoScreenId, DemoScreenDefinition> = {
  screen1: {
    id: "screen1",
    specLabel: "Screen 1",
    title: "Product Concept & Information Architecture",
    stage: "intent",
    lens: "review",
    statusLabel: "Concept board",
    pendingLabel: "Concept",
    workspace: <ProductConceptBoard />,
    standalone: true,
    smokeText: [
      "Product Concept & Information Architecture",
      "AI-assisted podcast editing",
      "Intent",
      "Indexing",
      "Proposal",
      "Evidence & Feedback Loop",
      "Information architecture",
      "Transparent",
      "Primary workspace model",
      "Workflow lenses",
    ],
  },
  screen2: {
    id: "screen2",
    specLabel: "Screen 2",
    title: "Main Desktop Workspace",
    stage: "proposal",
    lens: "review",
    statusLabel: "Awaiting review",
    pendingLabel: "12 pending",
    smokeText: [
      "Agent Command",
      "Podcast Tightening v1",
      "Proposal Inspector",
      "Timeline",
      "Transcript",
      "Evidence",
    ],
  },
  screen3: {
    id: "screen3",
    specLabel: "Screen 3",
    title: "Agent Proposal Review",
    stage: "review",
    lens: "review",
    statusLabel: "Reviewing clip pack",
    pendingLabel: "5 proposals",
    commandRail: screen3CommandRail,
    workspace: (
      <BatchReviewSurface
        commands={batchCommands}
        proposals={batchProposals}
        selectedProposalId="short-1"
        beforeFrame={<DemoImage src={podcastBefore} label="Source episode" />}
        afterFrame={<DemoImage src={podcastAfter} label="Proposed short clip" />}
        insights={{ pending: 4, accepted: 1, rejected: 0, avgConfidence: 0.87, riskLow: 3, riskMedium: 2, riskHigh: 0 }}
      />
    ),
    smokeText: [
      "Agent command history",
      "Proposed changes",
      "Short 01",
      "Before / After",
      "Batch insights",
      "Revise with prompt",
    ],
  },
  screen4: {
    id: "screen4",
    specLabel: "Screen 4",
    title: "Timeline / Transcript Hybrid",
    stage: "review",
    lens: "review",
    statusLabel: "Semantic timeline",
    pendingLabel: "12 pending",
    workspace: <SemanticTimelineWorkspace />,
    smokeText: [
      "Selected sentence",
      "Timeline / Transcript Hybrid",
      "Review · Transcript",
      "What this cut does",
      "Pending trim",
      "Keep this pause",
      "Edit around selection",
    ],
  },
  screen5: {
    id: "screen5",
    specLabel: "Screen 5",
    title: "Cut / Proposal Inspector",
    stage: "review",
    lens: "review",
    statusLabel: "Inspecting cut",
    pendingLabel: "Cut 12",
    workspace: <CutInspectorWorkspace />,
    smokeText: [
      "Cut / Proposal Inspector",
      "CUT 12 · L-cut",
      "Current timeline",
      "Proposed timeline",
      "Render output context",
      "Compare alternatives",
      "Inspect deeper",
    ],
  },
  screen6: {
    id: "screen6",
    specLabel: "Screen 6",
    title: "Import / Indexing State",
    stage: "indexing",
    lens: "index",
    statusLabel: "Indexing locally",
    pendingLabel: "9 signals",
    workspace: (
      <IndexingDashboard
        projectName="Interview_A"
        sourceCount={9}
        deliveryTarget="YouTube 16:9 (1920x1080)"
        media={indexingMedia}
        tasks={indexingTasks}
        structurePreview={{
          duration: "00:42:11",
          scenes: 31,
          segments: 126,
          speakers: 2,
          transcriptPercent: 78,
        }}
        system={{ cpu: 62, diskFreeGB: 1242.4, tempC: 61 }}
        ready={false}
      />
    ),
    smokeText: [
      "Add media",
      "Add files",
      "Add URL",
      "Indexing pipeline",
      "Transcripts",
      "Speaker diarization",
      "Indexing in progress",
      "Ask agent for first cut",
    ],
  },
  screen7: {
    id: "screen7",
    specLabel: "Screen 7",
    title: "Delivery / Preflight State",
    stage: "deliver",
    lens: "delivery",
    statusLabel: "Preflight open",
    pendingLabel: "2 warnings",
    workspace: (
      <DeliverySurface
        targets={deliveryTargets}
        findings={preflightFindings}
        summary={deliverySummary}
        onAgentRepair={() => undefined}
      />
    ),
    smokeText: [
      "Targets",
      "YouTube",
      "TikTok",
      "Preflight",
      "Render summary",
      "Delivery confidence",
      "Export now",
    ],
  },
  screen8: {
    id: "screen8",
    specLabel: "Screen 8",
    title: "Empty / Loading / Error States",
    stage: "intent",
    lens: "import",
    statusLabel: "System states",
    workspace: <SystemStateBoard />,
    smokeText: [
      "No media imported",
      "Indexing media",
      "Review transitions",
      "Proposal generation is blocked",
      "Repair with agent",
    ],
  },
  screen9: {
    id: "screen9",
    specLabel: "Screen 9",
    title: "Component System",
    stage: "review",
    lens: "review",
    statusLabel: "Component system",
    pendingLabel: "Tokens",
    workspace: <ComponentSystemBoard />,
    smokeText: [
      "Component System",
      "Proposal cards",
      "Timeline change markers",
      "Transcript segments",
      "Agent status",
      "Evidence chips",
      "Accept / Reject / Revise",
      "Media / indexing status",
      "Render / preflight findings",
      "Semantic color palette",
    ],
  },
};

const conceptStages: Array<{
  label: string;
  description: string;
  color: string;
  icon: LucideIcon;
}> = [
  { label: "Intent", description: "Define the goal and constraints in plain language.", color: "var(--color-success)", icon: MessageSquareText },
  { label: "Indexing", description: "Index media and generate rich understanding.", color: "var(--color-info)", icon: Database },
  { label: "Proposal", description: "Propose a walkthrough with edits and rationale.", color: "var(--color-warning)", icon: Sparkles },
  { label: "Review", description: "Review with evidence and full context.", color: "var(--color-brand-secondary)", icon: Eye },
  { label: "Revise", description: "Refine instructions or accept changes selectively.", color: "var(--color-brand-purple)", icon: SlidersHorizontal },
  { label: "Deliver", description: "Export with confidence, captions, and assets.", color: "var(--color-success)", icon: PackageCheck },
];

const informationArchitecture = [
  ["Project", "Projects", "Episodes", "Seasons", "Assets", "Team", "Activity"],
  ["Workspace", "Command Rail", "Timeline", "Transcript", "Selects", "Evidence", "Inspector"],
  ["Agent", "Intent", "Plan", "Actions", "Suggestions", "Activity Log", "Constraints"],
  ["Media", "Sources", "Clips", "Sequences", "Audio", "Images", "Documents"],
  ["Review", "Proposals", "Changes", "Evidence", "Alternatives", "Notes", "Compare"],
  ["Deliver", "Exports", "Packages", "Captions", "Shorts", "Destinations", "History"],
  ["Settings", "Preferences", "Models", "Indexing", "Storage", "Integrations", "Team"],
];

const conceptPrinciples: Array<{ title: string; description: string; icon: LucideIcon }> = [
  { title: "Transparent", description: "Every edit explains what changed and why.", icon: Eye },
  { title: "Reviewable", description: "Human approval stays between proposal and output.", icon: ShieldCheck },
  { title: "Staged Edits", description: "Workflow progress is visible and reversible.", icon: GitBranch },
  { title: "Evidence-Backed", description: "Suggestions cite transcript, audio, and visual signals.", icon: FileText },
  { title: "Local-First", description: "Media understanding begins on the workstation.", icon: Database },
  { title: "Terminal-Aware", description: "Desktop review stays connected to agent-native tools.", icon: Terminal },
];

function ProductConceptBoard() {
  return (
    <div className="flex h-full min-h-0 items-center justify-center overflow-hidden bg-[#070B10] p-3">
      <div
        className="grid aspect-[4/3] max-h-full max-w-full grid-rows-[54px_130px_minmax(0,1fr)_126px_48px] gap-3 overflow-hidden rounded-[var(--radius-lg)] border border-[var(--color-border-subtle)] bg-[#070B10] p-4 shadow-[0_24px_80px_rgba(0,0,0,0.42)]"
        style={{ width: "min(100%, calc((100vh - 24px) * 4 / 3))" }}
      >
        <header className="flex min-h-0 items-start justify-between gap-6 border-b border-[var(--color-border-subtle)] pb-3">
          <Stack gap="1" className="min-w-0">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-brand-secondary)]">
              Awidat · Product Concept & Information Architecture
            </span>
            <h1 className="text-[20px] font-semibold leading-tight text-[var(--color-text-primary)]">
              AI-assisted podcast editing that keeps humans in review.
            </h1>
          </Stack>
          <span className="shrink-0 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-1.5 text-right font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
            Awidat · v0.1 Concept · Internal Design Review
          </span>
        </header>

        <section className="grid min-h-0 grid-cols-[1.24fr_0.76fr] gap-3">
          <div className="relative rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
            <div className="absolute bottom-4 left-[8%] right-[8%] border-t border-dashed border-[rgba(56,189,248,0.45)]" />
            <div className="absolute bottom-1 left-1/2 z-20 -translate-x-1/2 rounded-full border border-[rgba(56,189,248,0.45)] bg-[#07111B] px-2.5 py-0.5 text-[9px] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-brand-secondary)]">
              Evidence & Feedback Loop
            </div>
            <div className="mb-2 flex items-baseline justify-between">
              <span className="text-[var(--text-label)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)]">
                Human-in-the-loop editing
              </span>
              <span className="font-mono text-[9px] text-[var(--color-text-muted)]">Intent → Indexing → Proposal → Review → Revise → Deliver</span>
            </div>
            <div className="grid grid-cols-6 gap-1.5 pb-5">
              {conceptStages.map((stage) => {
                const Icon = stage.icon;
                return (
                  <div key={stage.label} className="relative min-w-0 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
                    <Stack gap="1">
                      <span
                        className="flex h-6 w-6 items-center justify-center rounded-[var(--radius-xs)] border"
                        style={{ borderColor: stage.color, color: stage.color, background: "rgba(15, 23, 42, 0.5)" }}
                      >
                        <Icon className="h-3.5 w-3.5 stroke-[1.75]" />
                      </span>
                      <span className="text-[10px] font-semibold leading-3" style={{ color: stage.color }}>
                        {stage.label}
                      </span>
                      <span className="line-clamp-3 text-[9px] leading-3 text-[var(--color-text-muted)]">
                        {stage.description}
                      </span>
                    </Stack>
                  </div>
                );
              })}
            </div>
          </div>

          <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
            <Stack gap="2" className="h-full min-h-0">
              <ConceptSectionHeader title="Workspace shell anatomy" eyebrow="review cockpit" />
              <div className="grid flex-1 grid-cols-[0.62fr_1.28fr_0.78fr] grid-rows-[1fr_0.48fr] gap-1.5 text-[9px] font-semibold uppercase tracking-[var(--text-label--letter-spacing)]">
                <ConceptDiagramCell icon={Bot} label="Command rail" color="var(--color-success)" className="row-span-2" />
                <ConceptDiagramCell icon={LayoutDashboard} label="Preview / review" color="var(--color-info)" />
                <ConceptDiagramCell icon={PanelRight} label="Inspector" color="var(--color-brand-purple)" className="row-span-2" />
                <ConceptDiagramCell icon={Layers3} label="Timeline + transcript + diff" color="var(--color-warning)" />
              </div>
              <div className="grid grid-cols-3 gap-1.5">
                {["Current", "Proposed", "Render"].map((label, index) => (
                  <div key={label} className="rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1">
                    <span className="font-mono text-[9px] text-[var(--color-text-muted)]">0{index + 1}</span>
                    <span className="ml-1.5 text-[9px] font-semibold uppercase text-[var(--color-text-secondary)]">{label}</span>
                  </div>
                ))}
              </div>
            </Stack>
          </div>
        </section>

        <main className="grid min-h-0 grid-cols-[1.18fr_0.82fr] gap-3">
          <section className="min-h-0 rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
            <Stack gap="2" className="h-full min-h-0">
              <ConceptSectionHeader title="Information architecture" eyebrow="Product map" />
              <div className="grid grid-cols-7 items-stretch gap-1.5">
                {informationArchitecture.map(([title, ...items]) => (
                  <div key={title} className="min-w-0 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-1.5">
                    <Stack gap="1">
                      <span className="text-[10px] font-semibold leading-3 text-[var(--color-text-primary)]">{title}</span>
                      {items.map((item) => (
                        <span key={item} className="truncate text-[9px] leading-3 text-[var(--color-text-muted)]">
                          {item}
                        </span>
                      ))}
                    </Stack>
                  </div>
                ))}
              </div>
              <div className="grid min-h-0 flex-1 grid-cols-[0.9fr_1.1fr] gap-2 border-t border-[var(--color-border-subtle)] pt-2">
                <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
                  <Stack gap="1">
                    <span className="text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-brand-secondary)]">
                      Agent-native object
                    </span>
                    {[
                      ["Intent", "Plain-language goal and constraints"],
                      ["Proposal", "Pending edit package with rationale"],
                      ["Evidence", "Transcript, audio, visual, scene signals"],
                      ["Decision", "Accept, reject, revise before render"],
                    ].map(([label, detail]) => (
                      <Inline key={label} gap="2" align="start" className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1">
                        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--color-brand)] mt-1.5" />
                        <Stack gap="0" className="min-w-0">
                          <span className="text-[10px] font-semibold leading-3 text-[var(--color-text-primary)]">{label}</span>
                          <span className="truncate text-[9px] leading-3 text-[var(--color-text-muted)]">{detail}</span>
                        </Stack>
                      </Inline>
                    ))}
                  </Stack>
                </div>
                <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
                  <Stack gap="1">
                    <span className="text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)]">
                      State handoff model
                    </span>
                    {[
                      ["No media", "Import or open a project"],
                      ["Indexing", "Signals become evidence"],
                      ["Proposal", "Agent suggests staged edits"],
                      ["Preflight", "Export quality is checked"],
                    ].map(([label, detail], index) => (
                      <div key={label} className="grid grid-cols-[18px_minmax(0,1fr)] gap-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1">
                        <span className="font-mono text-[9px] text-[var(--color-brand-secondary)]">{index + 1}</span>
                        <Stack gap="0" className="min-w-0">
                          <span className="text-[10px] font-semibold leading-3 text-[var(--color-text-primary)]">{label}</span>
                          <span className="truncate text-[9px] leading-3 text-[var(--color-text-muted)]">{detail}</span>
                        </Stack>
                      </div>
                    ))}
                  </Stack>
                </div>
              </div>
            </Stack>
          </section>

          <section className="grid min-h-0 grid-rows-[1fr_104px] gap-3">
            <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
              <Stack gap="2">
                <ConceptSectionHeader title="Core principles" eyebrow="Operating model" />
                <div className="grid grid-cols-2 gap-2">
                  {conceptPrinciples.map((principle) => {
                    const Icon = principle.icon;
                    return (
                      <div key={principle.title} className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-1.5">
                        <Inline gap="2" align="start">
                          <Icon className="mt-0.5 h-3 w-3 shrink-0 stroke-[1.75] text-[var(--color-brand-secondary)]" />
                          <Stack gap="1" className="min-w-0">
                            <span className="text-[10px] font-semibold leading-3 text-[var(--color-text-primary)]">{principle.title}</span>
                            <span className="line-clamp-2 text-[9px] leading-3 text-[var(--color-text-muted)]">{principle.description}</span>
                          </Stack>
                        </Inline>
                      </div>
                    );
                  })}
                </div>
              </Stack>
            </div>

            <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
              <Stack gap="2">
                <ConceptSectionHeader title="Primary workspace model" eyebrow="Spatial layout" />
                <div className="grid grid-cols-[1fr_1.35fr_1fr] grid-rows-[30px_30px] gap-1.5 text-center text-[9px] font-semibold uppercase tracking-[var(--text-label--letter-spacing)]">
                  <ConceptDiagramCell icon={Bot} label="Agent / Command Rail" color="var(--color-success)" className="row-span-2" />
                  <ConceptDiagramCell icon={LayoutDashboard} label="Preview / Review Surface" color="var(--color-info)" />
                  <ConceptDiagramCell icon={PanelRight} label="Proposal Inspector" color="var(--color-brand-purple)" className="row-span-2" />
                  <ConceptDiagramCell icon={Layers3} label="Timeline / Transcript Hybrid" color="var(--color-warning)" />
                </div>
              </Stack>
            </div>
          </section>
        </main>

        <section className="grid min-h-0 grid-cols-[0.98fr_1.02fr_0.88fr] gap-3">
          <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
            <ConceptSectionHeader title="Reviewable edit package" eyebrow="Shared object" />
            <div className="mt-2 grid grid-cols-2 gap-1.5">
              {["Intent", "Plan", "Edits", "Evidence", "Diff", "Decision"].map((label) => (
                <div key={label} className="rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1 text-[9px] font-semibold text-[var(--color-text-secondary)]">
                  {label}
                </div>
              ))}
            </div>
          </div>
          <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
            <ConceptSectionHeader title="Trust signals" eyebrow="Evidence first" />
            <div className="mt-2 grid grid-cols-3 gap-1.5">
              {[
                ["Transcript", "94%"],
                ["Audio", "81%"],
                ["Scene", "88%"],
                ["Speaker", "High"],
                ["Continuity", "Med"],
                ["Preflight", "Pass"],
              ].map(([label, value]) => (
                <div key={label} className="rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1">
                  <span className="block truncate text-[9px] text-[var(--color-text-muted)]">{label}</span>
                  <span className="block font-mono text-[10px] text-[var(--color-brand-secondary)]">{value}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2.5">
            <ConceptSectionHeader title="Review controls" eyebrow="Human gate" />
            <div className="mt-2 grid grid-cols-3 gap-1.5">
              {[
                ["Accept", "var(--color-success)"],
                ["Reject", "var(--color-danger)"],
                ["Revise", "var(--color-brand-secondary)"],
              ].map(([label, color]) => (
                <div key={label} className="rounded-[var(--radius-xs)] border bg-[var(--color-surface-card)] px-2 py-1 text-center text-[9px] font-semibold" style={{ borderColor: color, color }}>
                  {label}
                </div>
              ))}
            </div>
          </div>
        </section>

        <footer className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2">
          <Inline justify="between" align="baseline" className="mb-1.5">
            <span className="text-[var(--text-caption)] font-semibold text-[var(--color-text-primary)]">Workflow lenses</span>
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">Review active</span>
          </Inline>
          <div className="grid grid-cols-8 gap-1.5">
            {["Import", "Index", "Selects", "Review", "Captions", "Audio", "Color", "Delivery"].map((lens) => (
              <div
                key={lens}
                className="rounded-[var(--radius-sm)] border px-2 py-1 text-center text-[10px] font-semibold"
                style={{
                  borderColor: lens === "Review" ? "var(--color-border-active)" : "var(--color-border-subtle)",
                  background: lens === "Review" ? "var(--color-surface-selected)" : "var(--color-surface-card)",
                  color: lens === "Review" ? "var(--color-brand-secondary)" : "var(--color-text-secondary)",
                }}
              >
                {lens}
              </div>
            ))}
          </div>
        </footer>
      </div>
    </div>
  );
}

function ConceptSectionHeader({ title, eyebrow }: { title: string; eyebrow: string }) {
  return (
    <Inline justify="between" align="baseline" gap="3">
      <h2 className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">{title}</h2>
      <span className="shrink-0 text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {eyebrow}
      </span>
    </Inline>
  );
}

function ConceptDiagramCell({
  icon: Icon,
  label,
  color,
  className,
}: {
  icon: LucideIcon;
  label: string;
  color: string;
  className?: string;
}) {
  return (
    <div
      className={`flex min-w-0 items-center justify-center gap-2 rounded-[var(--radius-sm)] border border-dashed bg-[var(--color-surface-card)] px-2 ${className ?? ""}`}
      style={{ borderColor: color, color }}
    >
      <Icon className="h-3.5 w-3.5 shrink-0 stroke-[1.75]" />
      <span className="text-center text-[10px] leading-3">{label}</span>
    </div>
  );
}

function ComponentSystemBoard() {
  return (
    <div className="grid h-full min-h-0 content-start grid-rows-[auto_112px] gap-3 bg-[var(--color-surface-app)] p-4">
      <div className="grid min-h-0 grid-cols-[1.05fr_0.95fr_0.95fr] gap-3">
        <Stack gap="3" className="min-h-0">
          <ComponentPanel title="Proposal cards" eyebrow="Variants">
            <div className="grid grid-cols-2 gap-2">
              {[
                ["Cut 07 · J-cut", "pending", "00:06:35 → 00:06:42", "88%", "Low"],
                ["Cut 08 · B-roll bridge", "accepted", "00:08:12 → 00:08:39", "92%", "Medium"],
                ["Cut 09 · False start", "rejected", "00:11:03 → 00:11:09", "74%", "High"],
                ["Cut 10 · Revised hook", "revised", "00:14:02 → 00:14:37", "81%", "Low"],
              ].map(([title, status, range, confidence, risk]) => (
                <div key={title} className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
                  <Stack gap="1">
                    <Inline justify="between" align="center" gap="2">
                      <span className="truncate text-[var(--text-caption)] font-semibold text-[var(--color-text-primary)]">{title}</span>
                      <Pill status={status as PillStatus} dot={false}>{status}</Pill>
                    </Inline>
                    <span className="font-mono text-[10px] text-[var(--color-text-muted)]">{range}</span>
                    <span className="text-[10px] leading-3 text-[var(--color-text-secondary)]">Remove filler phrase while preserving speaker cadence.</span>
                    <Inline justify="between">
                      <span className="text-[10px] text-[var(--color-text-muted)]">Confidence {confidence}</span>
                      <span className="text-[10px] text-[var(--color-text-muted)]">Risk {risk}</span>
                    </Inline>
                  </Stack>
                </div>
              ))}
            </div>
          </ComponentPanel>

          <ComponentPanel title="Transcript segments" eyebrow="States">
            <div className="grid gap-1.5">
              {[
                ["neutral", "A", "00:01:23", "So the short answer is yes, but the long answer is nuanced."],
                ["selected", "J", "00:01:31", "Yeah, exactly. We committed to the tighter opener."],
                ["accepted", "A", "00:01:42", "Right, because users are paying attention."],
                ["rejected", "J", "00:02:08", "Sorry, that did not come out right."],
              ].map(([state, speaker, time, text]) => (
                <div
                  key={`${state}-${time}`}
                  className="grid grid-cols-[28px_58px_minmax(0,1fr)_72px] items-center gap-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1.5"
                >
                  <span className="font-mono text-[10px] text-[var(--color-brand-secondary)]">{speaker}</span>
                  <span className="font-mono text-[10px] text-[var(--color-text-muted)]">{time}</span>
                  <span className="truncate text-[var(--text-caption)] text-[var(--color-text-secondary)]">{text}</span>
                  <Pill status={state === "neutral" ? "neutral" : (state as PillStatus)} dot={false}>{state}</Pill>
                </div>
              ))}
            </div>
          </ComponentPanel>
        </Stack>

        <Stack gap="3" className="min-h-0">
          <ComponentPanel title="Timeline change markers" eyebrow="Scrubbers / tracks / evidence">
            <Stack gap="3">
              <Inline gap="3" align="center" wrap="wrap">
                <TimelineMarker kind="accepted" label="accepted" />
                <TimelineMarker kind="rejected" label="rejected" />
                <TimelineMarker kind="pending" label="pending" />
                <TimelineMarker kind="warning" label="warning" />
                <TimelineMarker kind="info" label="info" />
                <TimelineMarker kind="skipped" label="skipped" />
              </Inline>
              <div className="relative h-16 overflow-hidden rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
                <div className="absolute inset-x-3 top-1/2 h-2 -translate-y-1/2 rounded-full bg-[var(--color-surface-card)]" />
                {["accepted", "pending", "warning", "info", "rejected"].map((kind, index) => (
                  <div key={kind} className="absolute top-1/2 -translate-y-1/2" style={{ left: `${18 + index * 16}%` }}>
                    <TimelineMarker kind={kind as never} />
                  </div>
                ))}
                <TimelineMarker kind="selected" className="absolute left-[43%] top-0" />
              </div>
            </Stack>
          </ComponentPanel>

          <ComponentPanel title="Evidence chips" eyebrow="Signals">
            <Inline gap="2" wrap="wrap">
              <EvidenceChip icon={<FileText />} label="Transcript match" confidence={0.94} />
              <EvidenceChip icon={<AudioWaveform />} label="Audio energy" confidence={0.81} />
              <EvidenceChip icon={<Users />} label="Speaker handoff" />
              <EvidenceChip icon={<VolumeX />} label="Silence detected" confidence={0.67} />
              <EvidenceChip icon={<SearchCheck />} label="Pacing deviation" />
              <EvidenceChip icon={<Captions />} label="Keyword density" confidence={0.53} />
            </Inline>
          </ComponentPanel>

          <ComponentPanel title="Confidence and risk indicators" eyebrow="Scoring">
            <Inline gap="4" align="center" wrap="wrap">
              <ConfidenceRing score={0.92} />
              <ConfidenceRing score={0.67} />
              <ConfidenceRing score={0.41} />
              <RiskIndicator level="low" />
              <RiskIndicator level="medium" />
              <RiskIndicator level="high" />
            </Inline>
            <Stack gap="1" className="mt-2">
              <ConfidenceMeter score={0.92} label="High 92%" size="sm" />
              <ConfidenceMeter score={0.41} label="Low 41%" size="sm" />
            </Stack>
          </ComponentPanel>
        </Stack>

        <Stack gap="3" className="min-h-0">
          <ComponentPanel title="Agent status" eyebrow="Runtime">
            <div className="grid grid-cols-2 gap-2">
              <AgentStatusBadge status="online" detail="All systems operational" />
              <AgentStatusBadge status="analyzing" detail="Processing media" />
              <AgentStatusBadge status="generating" detail="Drafting proposals" />
              <AgentStatusBadge status="awaiting-review" detail="12 pending changes" />
              <AgentStatusBadge status="idle" detail="Waiting for content" />
              <AgentStatusBadge status="offline" detail="Connection lost" />
            </div>
          </ComponentPanel>

          <ComponentPanel title="Accept / Reject / Revise controls" eyebrow="Decision safety">
            <Stack gap="2">
              <ReviewActions onAccept={() => undefined} onReject={() => undefined} onRevise={() => undefined} onRepair={() => undefined} showRepair />
              <span className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-muted)]">
                Accept applies the change. Reject keeps it out of the proposed timeline. Revise scopes the command input to the selected change.
              </span>
            </Stack>
          </ComponentPanel>

          <ComponentPanel title="Media / indexing status" eyebrow="Import pipeline">
            <Stack gap="2">
              <MediaStatusRow thumbnail={podcastThumb01} title="A_CAM_Main.mp4" detail="Imported · 4.2 GB · 00:42:11" status="imported" />
              <MediaStatusRow icon={<AudioWaveform />} title="Podcast_Master.wav" detail="Processing loudness and silence" status="processing" progress={42} />
              <MediaStatusRow icon={<Database />} title="Color analysis" detail="Failed · retry required" status="failed" />
              <MediaStatusRow icon={<Captions />} title="Questions.xlsx" detail="Missing from source folder" status="missing" />
            </Stack>
          </ComponentPanel>
        </Stack>
      </div>

      <div className="grid min-h-0 grid-cols-[1.2fr_0.8fr_0.9fr] gap-3">
        <ComponentPanel title="Render / preflight findings" eyebrow="Delivery checks">
          <div className="grid grid-cols-2 gap-2">
            {[
              ["pass", "Loudness within YouTube target", "00:00:00", "Master audio"],
              ["warning", "Speaker overlap detected", "00:12:18", "Track 2"],
            ].map(([severity, message, time, asset]) => (
              <div key={message} className="grid grid-cols-[18px_minmax(0,1fr)_70px] items-center gap-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1.5">
                <span
                  className="h-2 w-2 rounded-full"
                  style={{ background: severity === "pass" ? "var(--color-success)" : "var(--color-warning)" }}
                />
                <Stack gap="0" className="min-w-0">
                  <span className="truncate text-[var(--text-caption)] text-[var(--color-text-primary)]">{message}</span>
                  <span className="truncate text-[10px] text-[var(--color-text-muted)]">{asset}</span>
                </Stack>
                <span className="font-mono text-[10px] text-[var(--color-text-muted)]">{time}</span>
              </div>
            ))}
          </div>
        </ComponentPanel>
        <ComponentPanel title="Semantic color palette" eyebrow="Tokens">
          <div className="grid grid-cols-4 gap-2">
            {[
              ["Success", "var(--color-success)"],
              ["Warning", "var(--color-warning)"],
              ["Danger", "var(--color-danger)"],
              ["Info", "var(--color-info)"],
              ["Purple", "var(--color-brand-purple)"],
              ["Speaker A", "var(--color-viz-speaker-a)"],
              ["Speaker B", "var(--color-viz-speaker-b)"],
              ["Pending", "var(--color-viz-pending)"],
            ].map(([label, color]) => (
              <Stack key={label} gap="1" align="center">
                <span className="h-4 w-4 rounded-full border border-white/10" style={{ backgroundColor: color }} />
                <span className="text-center text-[9px] text-[var(--color-text-muted)]">{label}</span>
              </Stack>
            ))}
          </div>
        </ComponentPanel>
        <ComponentPanel title="System rule summary" eyebrow="Shared components">
          <Inline gap="2" wrap="wrap">
            {["status pills", "evidence chips", "confidence bars", "risk indicators", "media rows", "preflight rows"].map((label) => (
              <span key={label} className="rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1 text-[10px] text-[var(--color-text-secondary)]">
                {label}
              </span>
            ))}
          </Inline>
        </ComponentPanel>
      </div>
    </div>
  );
}

function ComponentPanel({ title, eyebrow, children }: { title: string; eyebrow: string; children: ReactNode }) {
  return (
    <section className="min-w-0 rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-3">
      <Stack gap="3">
        <Inline justify="between" align="baseline" gap="3">
          <h2 className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">{title}</h2>
          <span className="shrink-0 text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            {eyebrow}
          </span>
        </Inline>
        {children}
      </Stack>
    </section>
  );
}

function SemanticTranscriptPanel({ segments }: { segments: ReviewTranscriptSegment[] }) {
  return (
    <div className="flex h-full min-h-0 flex-col bg-[var(--color-surface-app)]">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-[var(--color-border-subtle)] px-4">
        <Inline gap="2" align="center">
          <Eye className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand)]" />
          <span className="text-[var(--text-label)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)]">
            Review · Transcript
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">5 rows · evidence synced</span>
        </Inline>
        <Inline gap="2" align="center">
          {["Audio cues", "Notes", "Evidence"].map((label) => (
            <span key={label} className="rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
              {label}
            </span>
          ))}
        </Inline>
      </div>
      <div className="min-h-0 flex-1 overflow-hidden p-3">
        <div className="grid h-full grid-rows-[28px_repeat(5,minmax(0,1fr))] overflow-hidden rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]">
          <div className="grid grid-cols-[42px_82px_minmax(0,1fr)_112px_84px_88px] items-center gap-2 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-3 text-[10px] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
            <span>Speaker</span>
            <span>Time</span>
            <span>Transcript sentence</span>
            <span>Evidence tags</span>
            <span>Confidence</span>
            <span>Status</span>
          </div>
          {segments.map((seg) => {
            const status = seg.state === "default" ? "pending" : seg.state === "selected" ? "pending" : seg.state ?? "pending";
            return (
              <div
                key={seg.id}
                className={cn(
                  "grid min-h-0 grid-cols-[42px_82px_minmax(0,1fr)_112px_84px_88px] items-center gap-2 border-b border-[var(--color-border-subtle)] px-3 last:border-b-0",
                  seg.state === "selected"
                    ? "bg-[var(--color-surface-selected)] outline outline-1 outline-[var(--color-border-active)] -outline-offset-1"
                    : seg.state === "accepted"
                      ? "bg-[rgba(34,197,94,0.08)]"
                      : seg.state === "warning"
                        ? "bg-[rgba(245,158,11,0.06)]"
                        : "bg-[var(--color-surface-card)]",
                )}
              >
                <Inline gap="2" align="center">
                  <span
                    className="flex h-5 w-5 items-center justify-center rounded-full text-[10px] font-bold text-white"
                    style={{ background: seg.speakerColor ?? "var(--color-brand-secondary)" }}
                  >
                    {seg.speaker}
                  </span>
                </Inline>
                <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{seg.startTime}</span>
                <span className="min-w-0 truncate text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{seg.text}</span>
                <Inline gap="1" wrap="wrap" className="min-w-0 overflow-hidden">
                  {(seg.evidence ?? []).slice(0, 2).map((ev) => (
                    <span key={ev.id} className="rounded-[var(--radius-xs)] bg-[var(--color-surface-input)] px-1.5 py-0.5 text-[10px] text-[var(--color-text-secondary)]">
                      {ev.label}
                    </span>
                  ))}
                </Inline>
                <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                  {typeof seg.confidence === "number" ? `${Math.round(seg.confidence * 100)} High` : "—"}
                </span>
                <Pill status={status as PillStatus} dot={false}>{status}</Pill>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function SemanticAlignmentPanel() {
  return (
    <div className="grid h-full min-h-0 grid-rows-[38px_minmax(0,1fr)] bg-[var(--color-surface-panel)]">
      <div className="flex items-center justify-between border-b border-[var(--color-border-subtle)] px-4">
        <Inline gap="4" align="center">
          {["Timeline", "Transcript", "Selects 12", "Changes 25", "Evidence"].map((tab) => (
            <span
              key={tab}
              className={cn(
                "text-[var(--text-caption)] font-medium",
                tab === "Transcript" ? "text-[var(--color-brand-secondary)]" : "text-[var(--color-text-muted)]",
              )}
            >
              {tab}
            </span>
          ))}
        </Inline>
        <Inline gap="2" align="center">
          <span className="text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
            View
          </span>
          <span className="rounded-[var(--radius-xs)] bg-[var(--color-surface-selected)] px-2 py-1 text-[var(--text-caption)] text-[var(--color-text-primary)]">Proposed Timeline</span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Current Timeline</span>
        </Inline>
      </div>
      <div className="grid min-h-0 grid-cols-[92px_minmax(0,1fr)] gap-x-3 gap-y-1.5 px-4 py-3">
        {semanticLanes.map((lane) => (
          <SemanticLaneRow key={lane.id} lane={lane} />
        ))}
      </div>
    </div>
  );
}

function SemanticLaneRow({ lane }: { lane: ChannelLane }) {
  const rangeStart = 216;
  const total = 29;
  return (
    <>
      <span className="self-center text-[var(--text-label)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-secondary)]">
        {lane.label}
      </span>
      <div className="relative self-center h-3.5 rounded-[var(--radius-xs)] bg-[var(--color-surface-input)]">
        {lane.segments.map((segment, index) => {
          const left = ((segment.startS - rangeStart) / total) * 100;
          const width = ((segment.endS - segment.startS) / total) * 100;
          const color =
            "color" in segment && segment.color
              ? segment.color
              : lane.paletteKey === "speaker"
                ? segment.key === "1"
                  ? "rgba(168,85,247,0.75)"
                  : "rgba(59,130,246,0.75)"
                : lane.paletteKey === "confidence"
                  ? "rgba(34,197,94,0.7)"
                  : "rgba(56,189,248,0.4)";
          return (
            <span
              key={`${lane.id}-${index}`}
              className="absolute top-0 h-full"
              style={{
                left: `${Math.max(0, left)}%`,
                width: `${Math.max(1, width)}%`,
                background: color,
              }}
            />
          );
        })}
        <span className="absolute inset-y-[-3px] left-[30%] w-0.5 bg-[var(--color-brand-secondary)] shadow-[0_0_10px_var(--color-brand-secondary)]" />
      </div>
    </>
  );
}

function SemanticTimelineWorkspace() {
  return (
    <div className="grid h-full grid-cols-[280px_minmax(0,1fr)_320px] bg-[var(--color-surface-app)] min-h-0">
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-3 overflow-y-auto">
        <Stack gap="4">
          <Stack gap="2">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Selected sentence
            </span>
            <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-3">
              <Stack gap="2">
                <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                  03:41:12 — 03:46:08 (4.9s)
                </span>
                <Inline justify="between">
                  <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Speaker</span>
                  <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">J</span>
                </Inline>
                <Inline justify="between">
                  <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Confidence</span>
                  <Pill status="accepted" dot={false}>High</Pill>
                </Inline>
              </Stack>
            </div>
          </Stack>

          <Stack gap="2">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Change status
            </span>
            <div className="grid grid-cols-2 gap-2">
              <StatusTile label="Pending" value="12" tone="warning" />
              <StatusTile label="Accepted" value="8" tone="success" />
              <StatusTile label="Rejected" value="3" tone="danger" />
              <StatusTile label="Warnings" value="2" tone="warning" />
            </div>
          </Stack>

          <Stack gap="2">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Confidence overview
            </span>
            <ConfidenceMeter score={0.76} label="High 76%" size="sm" />
            <ConfidenceMeter score={0.18} label="Medium 18%" size="sm" />
            <ConfidenceMeter score={0.06} label="Low 6%" size="sm" />
          </Stack>

          <Stack gap="2">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Evidence for selection
            </span>
            {[
              "Detected 1.8s of low-energy filler (um, so) — High",
              "Pause of 0.9s exceeds threshold (>0.7s) — High",
              "No overlap with other speakers — High",
            ].map((item) => (
              <div key={item} className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-2 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                {item}
              </div>
            ))}
          </Stack>
        </Stack>
      </aside>

      <main className="grid min-h-0 grid-rows-[200px_minmax(0,1fr)_168px]">
        <section className="relative border-b border-[var(--color-border-subtle)] bg-black">
          <DemoImage src={podcastWide} label="Selected segment · 03:41:12 — 03:46:08 (4.9s)" />
          <div className="absolute inset-y-0 left-[42%] w-[18%] border-x-2 border-[var(--color-brand-secondary)] bg-[rgba(56,189,248,0.08)]" />
        </section>
        <SemanticTranscriptPanel segments={reviewSegments} />
        <section className="border-t border-[var(--color-border-subtle)]">
          <SemanticAlignmentPanel />
        </section>
      </main>

      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-4 overflow-y-auto">
        <Stack gap="4">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            What this cut does
          </span>
          <Stack gap="2">
            <KVLine label="Type" value="Pending trim" />
            <KVLine label="Impact" value="-4.9s" />
            <KVLine label="Why" value="Removed filler + long pause" />
            <KVLine label="Confidence" value="High" />
          </Stack>
          <p className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
            The selected sentence contains filler and a long pause before the next speaker. Trimming it keeps cadence while preserving the meaning of the exchange.
          </p>
          <Stack gap="2">
            <Button variant="secondary" size="sm">Keep this pause</Button>
            <Button variant="revise" size="sm">Edit around selection</Button>
          </Stack>
          <div className="border-t border-[var(--color-border-subtle)] pt-4">
            <Inline gap="2" wrap="wrap">
              {["Trim", "Remove", "Keep", "Split", "Zoom -", "Zoom +"].map((label) => (
                <Button key={label} variant="ghost" size="xs">{label}</Button>
              ))}
            </Inline>
          </div>
        </Stack>
      </aside>
    </div>
  );
}

function SystemStateBoard() {
  return (
    <div className="grid h-full grid-cols-3 gap-4 bg-[var(--color-surface-app)] p-5">
      <StateColumn index="1" label="Empty state" tone="default">
        <div className="flex flex-1 items-center justify-center">
          <Stack gap="5" align="center" className="w-full max-w-[360px] text-center">
            <span className="relative flex h-20 w-20 items-center justify-center rounded-[var(--radius-lg)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-brand-secondary)]">
              <Film className="h-9 w-9 stroke-[1.5]" />
              <span className="absolute -right-2 -top-2 flex h-7 w-7 items-center justify-center rounded-full border border-[var(--color-border-active)] bg-[var(--color-surface-selected)]">
                <Import className="h-3.5 w-3.5 stroke-[1.75]" />
              </span>
            </span>
            <Stack gap="2" align="center">
              <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
                No project open yet
              </span>
              <span className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
                No media imported. Import media or create a new project to get started with intelligent proposals and review.
              </span>
            </Stack>
            <Stack gap="2" className="w-full">
              <Button variant="primary" leadingIcon={<Import className="h-3.5 w-3.5 stroke-[1.75]" />}>
                Import media
              </Button>
              <Button variant="secondary" leadingIcon={<FolderOpen className="h-3.5 w-3.5 stroke-[1.75]" />}>
                Open project
              </Button>
              <Button variant="ghost" leadingIcon={<Play className="h-3.5 w-3.5 stroke-[1.75]" />}>
                Start with example
              </Button>
            </Stack>
            <Card padding="sm" tone="flat" className="w-full text-left">
              <span className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-muted)]">
                Awidat's agents will analyze your content and propose a tight, publish-ready edit.
              </span>
            </Card>
          </Stack>
        </div>
      </StateColumn>

      <StateColumn index="2" label="Loading state" tone="loading">
        <div className="flex flex-1 items-center justify-center">
          <Stack gap="4" align="center" className="w-full max-w-[380px] text-center">
            <span className="flex h-20 w-20 items-center justify-center rounded-full border border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.08)] text-[var(--color-warning)]">
              <Loader className="h-9 w-9 animate-spin stroke-[1.5]" />
            </span>
            <Stack gap="2" align="center">
              <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
                Generating proposal
              </span>
              <span className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
                Indexing media. Our agents are analyzing your media and building a tight, publish-ready edit.
              </span>
            </Stack>
            <Stack gap="2" className="w-full text-left">
              <Inline justify="between" align="baseline">
                <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">Overall progress</span>
                <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-warning)]">62%</span>
              </Inline>
              <div className="h-2 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
                <div className="h-full w-[62%] rounded-full bg-[var(--color-warning)]" />
              </div>
            </Stack>
            <Card padding="md" className="w-full text-left">
              <Stack gap="2">
                <Inline justify="between" align="center">
                  <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                    Current task
                  </span>
                  <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">00:00:18</span>
                </Inline>
                <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                  Detecting pauses & fillers
                </span>
                <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Audio analysis</span>
              </Stack>
            </Card>
            <Stack gap="2" className="w-full text-left">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Up next
              </span>
              {["Build assembly", "Review transitions", "Draft captions & transcripts", "Score confidence & suggest edits"].map((item) => (
                <Inline key={item} gap="2" align="center">
                  <Clock className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
                  <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{item}</span>
                </Inline>
              ))}
            </Stack>
          </Stack>
        </div>
      </StateColumn>

      <StateColumn index="3" label="Error state" tone="error">
        <div className="flex flex-1 items-center justify-center">
          <Stack gap="4" align="center" className="w-full max-w-[390px] text-center">
            <span className="flex h-20 w-20 items-center justify-center rounded-full border border-[rgba(239,68,68,0.55)] bg-[rgba(239,68,68,0.1)] text-[var(--color-danger)]">
              <CircleAlert className="h-9 w-9 stroke-[1.5]" />
            </span>
            <Stack gap="2" align="center">
              <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
                We hit a problem
              </span>
              <span className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
                Proposal generation is blocked. Awidat couldn't complete the preflight check because 3 media files are missing or offline.
              </span>
            </Stack>
            <Card padding="md" tone="danger" className="w-full text-left">
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Impact
                </span>
                <span className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
                  We can't build a complete timeline without these files.
                </span>
              </Stack>
            </Card>
            <Stack gap="2" className="w-full text-left">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                How to resolve
              </span>
              {["Reconnect or relink the missing files", "Let Awidat find replacements automatically"].map((item) => (
                <Inline key={item} gap="2" align="center">
                  <CircleCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-success)]" />
                  <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{item}</span>
                </Inline>
              ))}
            </Stack>
            <Inline gap="2" wrap="wrap" justify="center">
              <Button variant="repair" leadingIcon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75]" />}>
                Repair with agent
              </Button>
              <Button variant="secondary" leadingIcon={<RotateCw className="h-3.5 w-3.5 stroke-[1.75]" />}>
                Retry
              </Button>
              <Button variant="ghost">Show details</Button>
            </Inline>
          </Stack>
        </div>
      </StateColumn>
    </div>
  );
}

function StateColumn({
  index,
  label,
  tone,
  children,
}: {
  index: string;
  label: string;
  tone: "default" | "loading" | "error";
  children: ReactNode;
}) {
  const toneColor =
    tone === "error"
      ? "var(--color-danger)"
      : tone === "loading"
        ? "var(--color-warning)"
        : "var(--color-brand-secondary)";
  return (
    <section className="flex min-h-0 flex-col rounded-[var(--radius-lg)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-4">
      <Inline gap="3" align="center" className="shrink-0 border-b border-[var(--color-border-subtle)] pb-3">
        <span
          className="flex h-7 w-7 items-center justify-center rounded-[var(--radius-sm)] border font-mono text-[var(--text-caption)] font-semibold"
          style={{
            borderColor: toneColor,
            color: toneColor,
            background: "rgba(15, 23, 42, 0.5)",
          }}
        >
          {index}
        </span>
        <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
          {label}
        </span>
      </Inline>
      {children}
    </section>
  );
}

function CutInspectorWorkspace() {
  return (
    <div className="grid h-full grid-cols-[minmax(0,1fr)_420px] bg-[var(--color-surface-app)] min-h-0">
      <main className="grid min-h-0 grid-rows-[minmax(360px,1fr)_220px]">
        <section className="relative border-b border-[var(--color-border-subtle)] bg-black">
          <img src={podcastWide} alt="" className="h-full w-full object-cover opacity-90" />
          <div className="absolute inset-y-[12%] left-[46%] w-[14%] rounded-[var(--radius-sm)] border-2 border-[var(--color-brand-purple)] bg-[rgba(168,85,247,0.16)] shadow-[0_0_24px_rgba(168,85,247,0.24)]" />
          <div className="absolute inset-y-[8%] left-[53%] w-0.5 bg-[var(--color-viz-playhead)] shadow-[0_0_12px_var(--color-viz-playhead)]" />
          <div className="absolute left-[46%] top-[12%] -translate-y-full rounded-t-[var(--radius-xs)] border border-[rgba(168,85,247,0.6)] bg-[rgba(30,15,50,0.85)] px-2 py-1 text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-pill-reviewing-text)]">
            CUT 12 · L-cut
          </div>
          <div className="absolute bottom-4 left-4 rounded-[var(--radius-md)] border border-black/40 bg-black/70 px-3 py-2">
            <Stack gap="1">
              <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-white/60">
                Selected change region
              </span>
              <span className="font-mono text-[var(--text-body-sm)] text-white/90">
                00:06:42:11 — 00:06:45:02
              </span>
            </Stack>
          </div>
        </section>

        <section className="p-4">
          <Stack gap="3">
            <TimelineContextRow label="Current timeline" segments={["gap", "question", "pause", "answer"]} activeIndex={2} />
            <TimelineContextRow label="Proposed timeline" segments={["question", "l-cut", "answer"]} activeIndex={1} />
            <TimelineContextRow label="Render output context" segments={["clean intro", "handoff", "response"]} activeIndex={1} />
          </Stack>
        </section>
      </main>
      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] min-h-0">
        <ProposalInspector data={cutInspectorData} onAgentRepair={() => undefined} onInspectDeeper={() => undefined} />
      </aside>
    </div>
  );
}

function TimelineContextRow({
  label,
  segments,
  activeIndex,
}: {
  label: string;
  segments: string[];
  activeIndex: number;
}) {
  return (
    <div className="grid grid-cols-[160px_1fr] items-center gap-3">
      <span className="text-right text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <div className="flex h-11 gap-1 rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] p-1">
        {segments.map((segment, index) => (
          <div
            key={`${label}-${segment}`}
            className="flex min-w-0 flex-1 items-center justify-center rounded-[var(--radius-xs)] border px-2 text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)]"
            style={{
              borderColor:
                index === activeIndex
                  ? "var(--color-border-active)"
                  : "var(--color-border-subtle)",
              background:
                index === activeIndex
                  ? "rgba(56, 189, 248, 0.14)"
                  : "var(--color-surface-card)",
              color:
                index === activeIndex
                  ? "var(--color-brand-secondary-hover)"
                  : "var(--color-text-secondary)",
            }}
          >
            {segment}
          </div>
        ))}
      </div>
    </div>
  );
}

function StatusTile({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: "success" | "warning" | "danger";
}) {
  const color =
    tone === "success"
      ? "var(--color-success)"
      : tone === "warning"
        ? "var(--color-warning)"
        : "var(--color-danger)";
  return (
    <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
      <div className="font-mono text-[var(--text-h3)] font-semibold" style={{ color }}>{value}</div>
      <div className="text-[var(--text-caption)] text-[var(--color-text-muted)]">{label}</div>
    </div>
  );
}

function KVLine({ label, value }: { label: string; value: string }) {
  return (
    <Inline justify="between" align="baseline" gap="3">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="text-right text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{value}</span>
    </Inline>
  );
}

function DemoImage({ src, label }: { src: string; label: string }) {
  return (
    <figure className="relative h-full w-full">
      <img src={src} alt="" className="h-full w-full object-cover" />
      <figcaption className="absolute bottom-2 left-2 rounded-[var(--radius-xs)] border border-black/40 bg-black/60 px-2 py-1 text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-white/80">
        {label}
      </figcaption>
    </figure>
  );
}

// Keep these imports active for planned screen variants.
void Sparkles;
void RotateCw;
void podcastWide;
