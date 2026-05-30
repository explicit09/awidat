import {
  AudioWaveform,
  Captions,
  Clapperboard,
  Database,
  FileText,
  Import,
  SearchCheck,
  ShieldCheck,
  Sparkles,
  Upload,
  Users,
  VolumeX,
} from "lucide-react";
import wordmark from "../brand/awidat-wordmark.svg";
import {
  AgentStatusBadge,
  Button,
  Card,
  ConfidenceMeter,
  ConfidenceRing,
  Divider,
  EvidenceChip,
  IconButton,
  Inline,
  MediaStatusRow,
  PreflightFindingRow,
  ProposalCard,
  StatusPill,
  ReviewActions,
  RiskIndicator,
  Stack,
  TimelineMarker,
  TranscriptSegment,
} from "./index";

function thumb(label: string, bg = "#17212F", fg = "#9CA3AF") {
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 160 100'><rect width='160' height='100' fill='${bg}'/><circle cx='80' cy='50' r='14' fill='none' stroke='${fg}' stroke-width='1.5'/><polygon points='75,44 75,56 87,50' fill='${fg}'/><text x='80' y='86' font-family='Inter, sans-serif' font-size='9' fill='${fg}' text-anchor='middle' letter-spacing='0.04em'>${label}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

function Section({ title, children, columns = 1 }: { title: string; children: React.ReactNode; columns?: number }) {
  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {title}
      </h2>
      <div
        className="grid gap-3"
        style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
      >
        {children}
      </div>
    </section>
  );
}

export function Gallery() {
  return (
    <div className="min-h-screen bg-[var(--color-surface-app)] text-[var(--color-text-primary)] font-sans">
      <header className="border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)]">
        <div className="mx-auto max-w-[1400px] flex items-center justify-between px-6 h-[var(--layout-chrome-h)]">
          <Inline gap="3" align="center">
            <img src={wordmark} alt="Awidat" className="h-7" />
            <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
              UI Gallery · Phase 1
            </span>
          </Inline>
          <AgentStatusBadge status="online" detail="All systems operational" />
        </div>
      </header>

      <main className="mx-auto max-w-[1400px] px-6 py-8">
        <Stack gap="8">
          <Section title="Typography ramp">
            <Card padding="lg">
              <Stack gap="3">
                <span className="text-[length:var(--text-display)] leading-[var(--text-display--line-height)] tracking-[var(--text-display--letter-spacing)] font-bold">
                  Display 32 — Awidat
                </span>
                <span className="text-[length:var(--text-h1)] leading-[var(--text-h1--line-height)] tracking-[var(--text-h1--letter-spacing)] font-bold">
                  H1 24 — Proposal Review
                </span>
                <span className="text-[length:var(--text-h2)] leading-[var(--text-h2--line-height)] tracking-[var(--text-h2--letter-spacing)] font-semibold">
                  H2 18 — Proposed Timeline
                </span>
                <span className="text-[length:var(--text-h3)] leading-[var(--text-h3--line-height)] tracking-[var(--text-h3--letter-spacing)] font-semibold">
                  H3 15 — Cut 07 · J-cut
                </span>
                <span className="text-[length:var(--text-body)] leading-[var(--text-body--line-height)]">
                  Body 14 — Tightens the open by lifting the host’s welcome.
                </span>
                <span className="text-[length:var(--text-body-sm)] leading-[var(--text-body-sm--line-height)] text-[var(--color-text-secondary)]">
                  Body-small 13 — secondary description, table rows
                </span>
                <span className="text-[length:var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Label 12 — Agent Plan
                </span>
                <span className="text-[length:var(--text-caption)] tracking-[var(--text-caption--letter-spacing)] text-[var(--color-text-muted)]">
                  Caption 11 — metadata, helper text
                </span>
                <span className="font-mono text-[length:var(--text-timecode)] text-[var(--color-text-primary)]">
                  00:01:23.456 — Timecode (JetBrains Mono)
                </span>
              </Stack>
            </Card>
          </Section>

          <Section title="Surface stack">
            <Card padding="none" tone="flat">
              <div className="flex">
                {[
                  ["page", "var(--color-surface-page)"],
                  ["app", "var(--color-surface-app)"],
                  ["panel", "var(--color-surface-panel)"],
                  ["card", "var(--color-surface-card)"],
                  ["modal", "var(--color-surface-modal)"],
                  ["hover", "var(--color-surface-hover)"],
                  ["input", "var(--color-surface-input)"],
                  ["selected", "var(--color-surface-selected)"],
                ].map(([name, color]) => (
                  <div
                    key={name}
                    className="flex-1 h-20 flex items-end p-2 text-[var(--text-caption)] text-[var(--color-text-muted)] border-r border-[var(--color-border-subtle)] last:border-r-0"
                    style={{ backgroundColor: color }}
                  >
                    {name}
                  </div>
                ))}
              </div>
            </Card>
          </Section>

          <Section title="Buttons" columns={1}>
            <Card padding="lg">
              <Stack gap="4">
                <Inline gap="2" wrap="wrap">
                  <Button variant="primary">Accept selected</Button>
                  <Button variant="secondary">Inspect deeper</Button>
                  <Button variant="ghost">Cancel</Button>
                  <Button variant="danger">Discard</Button>
                </Inline>
                <Inline gap="2" wrap="wrap">
                  <Button variant="primary" size="sm">Accept</Button>
                  <Button variant="secondary" size="sm">More</Button>
                  <Button variant="ghost" size="sm">Skip</Button>
                  <Button variant="primary" size="xs">Tiny</Button>
                </Inline>
                <Inline gap="2" wrap="wrap">
                  <Button variant="primary" disabled>Disabled</Button>
                  <Button variant="secondary" disabled>Disabled</Button>
                </Inline>
                <Divider />
                <ReviewActions
                  onAccept={() => undefined}
                  onReject={() => undefined}
                  onRevise={() => undefined}
                  onRepair={() => undefined}
                  showRepair
                />
                <ReviewActions
                  size="sm"
                  onAccept={() => undefined}
                  onReject={() => undefined}
                  onRevise={() => undefined}
                />
              </Stack>
            </Card>
          </Section>

          <Section title="Status pills">
            <Card padding="lg">
              <Stack gap="3">
                <Inline gap="2" wrap="wrap">
                  <StatusPill family="job" state="idle" />
                  <StatusPill family="job" state="running" />
                  <StatusPill family="job" state="running" percent={56} label="Indexing" />
                  <StatusPill family="job" state="ready" />
                  <StatusPill family="job" state="failed" />
                </Inline>
                <Inline gap="2" wrap="wrap">
                  <StatusPill family="proposal" state="proposed" />
                  <StatusPill family="proposal" state="accepted" />
                  <StatusPill family="proposal" state="rejected" />
                  <StatusPill family="proposal" state="revised" />
                </Inline>
              </Stack>
            </Card>
          </Section>

          <Section title="Evidence chips">
            <Card padding="lg">
              <Inline gap="2" wrap="wrap">
                <EvidenceChip icon={<FileText />} label="Transcript match" confidence={0.94} />
                <EvidenceChip icon={<AudioWaveform />} label="Audio energy" confidence={0.81} />
                <EvidenceChip icon={<Users />} label="Speaker handoff" />
                <EvidenceChip icon={<VolumeX />} label="Silence detected" confidence={0.67} />
                <EvidenceChip icon={<SearchCheck />} label="Pacing deviation" />
                <EvidenceChip icon={<Captions />} label="Filler phrase" confidence={0.53} />
              </Inline>
            </Card>
          </Section>

          <Section title="Confidence & risk" columns={2}>
            <Card padding="lg">
              <Stack gap="4">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Confidence rings
                </span>
                <Inline gap="6" align="center" wrap="wrap">
                  <ConfidenceRing score={0.92} />
                  <ConfidenceRing score={0.67} />
                  <ConfidenceRing score={0.41} />
                  <ConfidenceRing score={0.15} />
                </Inline>
                <Divider />
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Confidence bars (inline use)
                </span>
                <ConfidenceMeter score={0.92} />
                <ConfidenceMeter score={0.67} />
                <ConfidenceMeter score={0.41} />
                <ConfidenceMeter score={0.15} />
              </Stack>
            </Card>
            <Card padding="lg">
              <Stack gap="4">
                <RiskIndicator level="low" />
                <RiskIndicator level="medium" />
                <RiskIndicator level="high" />
                <RiskIndicator level="very-high" />
              </Stack>
            </Card>
          </Section>

          <Section title="Agent status">
            <Card padding="lg">
              <Stack gap="2">
                <AgentStatusBadge status="online" detail="All systems operational" />
                <AgentStatusBadge status="analyzing" detail="Processing audio &amp; video" />
                <AgentStatusBadge status="generating" detail="Identifying potential edits" />
                <AgentStatusBadge status="awaiting-review" detail="12 pending changes" />
                <AgentStatusBadge status="idle" detail="Waiting for content" />
                <AgentStatusBadge status="offline" detail="Connection lost" />
              </Stack>
            </Card>
          </Section>

          <Section title="Proposal cards" columns={4}>
            <ProposalCard
              title="Cut 07 · J-cut"
              proposalState="proposed"
              statusLabel="Pending"
              timeRange="00:12:04 → 00:12:18"
              cutType="J-cut"
              explanation="Lifts host welcome by 0.8s under the guest reaction."
              confidence={0.84}
              risk="low"
              thumbnail={thumb("00:12:04")}
              onOverflow={() => undefined}
            />
            <ProposalCard
              title="Cut 12 · L-cut"
              proposalState="proposed"
              timeRange="00:18:42 → 00:18:51"
              cutType="L-cut"
              explanation="Extends speaker A audio under the cutaway."
              confidence={0.61}
              risk="medium"
              thumbnail={thumb("00:18:42")}
              selected
              onOverflow={() => undefined}
            />
            <ProposalCard
              title="Cut 23 · Filler"
              proposalState="rejected"
              timeRange="00:24:11 → 00:24:13"
              cutType="Filler removal"
              explanation="Removes 'um' but cuts mid-thought."
              confidence={0.31}
              risk="high"
              thumbnail={thumb("00:24:11")}
              onOverflow={() => undefined}
            />
            <ProposalCard
              title="Cut 31 · Pause"
              proposalState="accepted"
              timeRange="00:31:08 → 00:31:11"
              cutType="Tighten pause"
              explanation="Removes 2.4s pause; preserves natural breath."
              confidence={0.95}
              risk="low"
              thumbnail={thumb("00:31:08")}
              onOverflow={() => undefined}
            />
          </Section>

          <Section title="Transcript segments" columns={1}>
            <Card padding="lg">
              <Stack gap="2">
                <TranscriptSegment
                  speaker="Alex"
                  startTime="00:01:23"
                  text="So the short answer is, we’d be lying if we said it never nuanced."
                  confidence={0.92}
                  evidence={
                    <>
                      <EvidenceChip icon={<Captions />} label="Filler phrase" />
                      <EvidenceChip icon={<AudioWaveform />} label="Pause 0.6s" />
                    </>
                  }
                />
                <TranscriptSegment
                  speaker="Jordan"
                  speakerColor="var(--color-viz-speaker-b)"
                  startTime="00:01:31"
                  state="selected"
                  text="Yeah, exactly. We thought it through and we were like — let’s just commit."
                  confidence={0.88}
                />
                <TranscriptSegment
                  speaker="Alex"
                  startTime="00:01:42"
                  state="accepted"
                  text="Right, because users are paying attention to *how* you say it."
                  confidence={0.81}
                />
                <TranscriptSegment
                  speaker="Jordan"
                  speakerColor="var(--color-viz-speaker-b)"
                  startTime="00:01:55"
                  state="warning"
                  text="And um, uh, you know, like, that’s the thing about—"
                  confidence={0.52}
                  evidence={
                    <>
                      <EvidenceChip icon={<Captions />} label="High filler density" confidence={0.74} />
                    </>
                  }
                />
                <TranscriptSegment
                  speaker="Alex"
                  startTime="00:02:08"
                  state="rejected"
                  text="Sorry, that didn’t come out right. Let me start over."
                  confidence={0.46}
                />
              </Stack>
            </Card>
          </Section>

          <Section title="Timeline markers">
            <Card padding="lg">
              <Stack gap="4">
                <Inline gap="4" align="center">
                  <TimelineMarker kind="accepted" />
                  <TimelineMarker kind="rejected" />
                  <TimelineMarker kind="pending" />
                  <TimelineMarker kind="warning" />
                  <TimelineMarker kind="info" />
                  <TimelineMarker kind="skipped" />
                  <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                    accepted · rejected · pending · warning · info · skipped
                  </span>
                </Inline>
                <div
                  className="relative h-10 rounded-[var(--radius-sm)] bg-[var(--color-surface-input)] border border-[var(--color-border-subtle)] overflow-hidden"
                >
                  <div className="absolute left-[35%] top-0 h-full w-0.5" style={{ backgroundColor: "var(--color-brand-purple)", boxShadow: "0 0 12px var(--color-brand-purple)" }} />
                  <span className="absolute left-[35%] top-1/2 -translate-y-1/2 ml-2 text-[var(--text-caption)] text-[var(--color-text-secondary)] font-mono">
                    selected playhead
                  </span>
                </div>
              </Stack>
            </Card>
          </Section>

          <Section title="Media / indexing rows" columns={1}>
            <Card padding="lg">
              <Stack gap="2">
                <MediaStatusRow
                  thumbnail={thumb("ep_01.mp4")}
                  title="ep_01_host_camera.mp4"
                  detail="1080p · 23:42 · Whisper transcript ready"
                  status="indexed"
                />
                <MediaStatusRow
                  thumbnail={thumb("ep_01_g.mp4", "#1F2937")}
                  title="ep_01_guest_camera.mp4"
                  detail="3 scenes detected · audio analysis pending"
                  status="indexing"
                  progress={64}
                />
                <MediaStatusRow
                  icon={<AudioWaveform />}
                  title="Audio analysis"
                  detail="Loudness, silence, energy"
                  status="processing"
                  progress={32}
                />
                <MediaStatusRow
                  icon={<Users />}
                  title="Speaker diarization"
                  detail="2 speakers identified"
                  status="partial"
                />
                <MediaStatusRow
                  icon={<Captions />}
                  title="Caption readiness"
                  detail="Awaiting transcript"
                  status="missing"
                />
                <MediaStatusRow
                  icon={<Database />}
                  title="Color analysis"
                  detail="LUTs cached"
                  status="failed"
                />
              </Stack>
            </Card>
          </Section>

          <Section title="Preflight findings" columns={1}>
            <Card padding="lg">
              <Stack gap="2">
                <PreflightFindingRow
                  severity="pass"
                  time="00:00:00"
                  message="Loudness within YouTube target (-14 LUFS)"
                  asset="Master audio"
                />
                <PreflightFindingRow
                  severity="info"
                  time="00:04:12"
                  message="Caption file generated (SRT + VTT)"
                  asset="captions/episode.srt"
                />
                <PreflightFindingRow
                  severity="warning"
                  time="00:12:18"
                  message="Speaker A overlaps speaker B for 0.4s"
                  asset="Track 2"
                  suggestedFix="Trim 0.4s from the start of speaker B"
                />
                <PreflightFindingRow
                  severity="error"
                  time="00:24:05"
                  message="Subtitle exceeds 42 chars on 2 lines"
                  asset="captions/episode.srt"
                  suggestedFix="Split the line at the natural pause"
                />
                <PreflightFindingRow
                  severity="failure"
                  message="Render output failed: missing source media"
                  asset="podcast-final.mp4"
                  suggestedFix="Relink media and retry render"
                />
              </Stack>
            </Card>
          </Section>

          <Section title="Icon coverage (Lucide)">
            <Card padding="lg">
              <Inline gap="3" wrap="wrap">
                {[
                  { I: Sparkles, label: "Agent" },
                  { I: Database, label: "Index" },
                  { I: Captions, label: "Captions" },
                  { I: AudioWaveform, label: "Audio" },
                  { I: Clapperboard, label: "Scene" },
                  { I: Users, label: "Speakers" },
                  { I: ShieldCheck, label: "Preflight" },
                  { I: SearchCheck, label: "Evidence" },
                  { I: Import, label: "Import" },
                  { I: Upload, label: "Deliver" },
                ].map(({ I, label }) => (
                  <IconButton key={label} icon={<I />} label={label} />
                ))}
              </Inline>
            </Card>
          </Section>
        </Stack>
      </main>
    </div>
  );
}
