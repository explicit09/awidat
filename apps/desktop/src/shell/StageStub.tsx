import {
  Database,
  Eye,
  MessageSquareText,
  SlidersHorizontal,
  Upload,
  type LucideIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { Button, Card, Inline, Stack } from "../ui";
import type { Stage } from "../state";

/**
 * StageStub — the "this stage isn't built yet" placeholder used by all
 * stages except `proposal` (which is fully realized in Phase 2).
 *
 * Follows the design spec's empty-state pattern (Screen 8): a centered
 * card with the stage glyph, a title, a one-line value statement, and
 * up to two CTAs. The CTAs route the user back to the Proposal stage
 * (the only working surface) for now.
 */

const STAGE_META: Record<
  Stage,
  { Icon: LucideIcon; title: string; value: string; primary: string; secondary?: string }
> = {
  intent: {
    Icon: MessageSquareText,
    title: "Intent",
    value:
      "Tell Awidat what you want and the agent indexes, proposes, and explains. Phase 4 surfaces this stage with the full command-driven start.",
    primary: "Open Proposal stage",
  },
  indexing: {
    Icon: Database,
    title: "Indexing",
    value:
      "Watch transcripts, scenes, audio analysis, motion, color, and speaker diarization complete in parallel. Phase 4 builds this dashboard.",
    primary: "Open Proposal stage",
    secondary: "Read the spec",
  },
  proposal: {
    // Never rendered — proposal stage has its own surface.
    Icon: MessageSquareText,
    title: "Proposal",
    value: "Proposal stage is the working surface — this stub is unreachable.",
    primary: "Continue",
  },
  review: {
    Icon: Eye,
    title: "Review",
    value:
      "The transcript becomes a first-class editing surface — sentence-level cuts, evidence chips, channel lanes. Phase 3 builds this lens.",
    primary: "Open Proposal stage",
  },
  revise: {
    Icon: SlidersHorizontal,
    title: "Revise",
    value:
      "Agent-mediated correction: ask for changes in natural language, see the diff, and step through the revision history. Phase 3 lands this.",
    primary: "Open Proposal stage",
  },
  deliver: {
    Icon: Upload,
    title: "Deliver",
    value:
      "Per-target preflight, safe-area guides, and platform packaging for YouTube, TikTok, Instagram, captions, and cover art. Phase 4 builds this.",
    primary: "Open Proposal stage",
  },
};

export type StageStubProps = {
  stage: Stage;
  onPrimaryAction?: () => void;
  onSecondaryAction?: () => void;
  /** Custom slot for extra child content (e.g. a link list, status info). */
  children?: ReactNode;
};

export function StageStub({ stage, onPrimaryAction, onSecondaryAction, children }: StageStubProps) {
  const meta = STAGE_META[stage];
  const Icon = meta.Icon;
  return (
    <div className="flex h-full w-full items-center justify-center p-8">
      <Card tone="elevated" padding="lg" className="w-full max-w-[480px]">
        <Stack gap="4" align="center" className="text-center">
          <span
            className="flex h-14 w-14 items-center justify-center rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-brand)]"
            aria-hidden
          >
            <Icon className="h-7 w-7 stroke-[1.5]" />
          </span>
          <Stack gap="2" align="center" className="text-center">
            <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
              {meta.title}
            </span>
            <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-relaxed">
              {meta.value}
            </span>
          </Stack>
          {children}
          <Inline gap="2" align="center">
            <Button variant="primary" onClick={onPrimaryAction}>
              {meta.primary}
            </Button>
            {meta.secondary ? (
              <Button variant="ghost" onClick={onSecondaryAction}>
                {meta.secondary}
              </Button>
            ) : null}
          </Inline>
        </Stack>
      </Card>
    </div>
  );
}
