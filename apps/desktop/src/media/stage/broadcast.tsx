import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { TimelineSnapshot } from "../../timeline/store";
import { projectAssetUrl } from "./motionScene";

type BroadcastOverlayConfig = NonNullable<TimelineSnapshot["broadcast_overlay"]>;
type BroadcastHost = BroadcastOverlayConfig["host_a"];
type BroadcastTimedEntry = BroadcastOverlayConfig["topics"][number];

export function TimelineBroadcastOverlay({
  overlay,
  timelineTime,
  projectRoot,
  resolveAssetUrl = projectAssetUrl,
  previewFrameSize,
}: {
  overlay: TimelineSnapshot["broadcast_overlay"];
  timelineTime: number;
  projectRoot: string | null;
  resolveAssetUrl?: (projectRoot: string | null, relPath: string | null) => string | null;
  previewFrameSize?: { width: number; height: number };
}) {
  if (!overlay?.enabled) return null;

  const style = overlay.style;
  const previewScale = responsiveBroadcastOverlayScale(previewFrameSize);
  const gold = normalizeCssHex(style.gold_hex, "#C9A028");
  const goldLight = normalizeCssHex(style.gold_light_hex, "#E8C040");
  const cyan = normalizeCssHex(style.cyan_hex, "#22D3EE");
  const navy = normalizeCssHex(style.dark_navy_hex, "#070D17");
  const inTitle = timelineTime >= 0 && timelineTime < style.title_visible_end;
  const inHostIntro =
    timelineTime >= style.host_intro_start && timelineTime < style.host_intro_end;
  const tickerEntries = broadcastTickerEntries(overlay);
  const activeChapter =
    overlay.chapters.length > 0
      ? activeChapterEntry(
          overlay.chapters,
          timelineTime,
          style.chapter_display_duration,
          Math.max(0, style.title_visible_end),
        )
      : null;
  const tickerPhase = broadcastTickerPhase(
    tickerEntries,
    timelineTime,
    style,
  );
  const sponsorText =
    overlay.sponsors.length > 0
      ? `${overlay.sponsors.join("   ◆   ")}   ◆`
      : overlay.show_name || overlay.template_name || "BROADCAST";
  const overlayStyleVars = {
    "--broadcast-preview-scale": previewScale,
    "--broadcast-name-bar-height": refHeightPercent(style.name_bar_height * previewScale),
    "--broadcast-ticker-height": refHeightPercent(style.ticker_height * previewScale),
    "--broadcast-host-strip-height": refHeightPercent(style.host_strip_height * previewScale),
    "--broadcast-ticker-label-width": refWidthPercent(680 * previewScale),
  } as React.CSSProperties;

  return (
    <div className="broadcast-overlay-layer" style={overlayStyleVars} aria-hidden="true">
      <BroadcastAssetPreloads
        overlay={overlay}
        projectRoot={projectRoot}
        resolveAssetUrl={resolveAssetUrl}
      />
      {overlay.short_form_mode ? (
        <div
          className="broadcast-short-brand-bar"
          style={{
            "--broadcast-navy": navy,
            "--broadcast-gold": gold,
          } as React.CSSProperties}
        >
          <BroadcastBrandLogo
            logoPath={overlay.brand_logo_path}
            projectRoot={projectRoot}
            resolveAssetUrl={resolveAssetUrl}
          />
          <strong>{(overlay.show_name || overlay.episode_title || "BROADCAST").toUpperCase()}</strong>
        </div>
      ) : (
        <>
      {inTitle && (
        <div
          className="broadcast-title-card"
          style={{
            "--broadcast-navy": navy,
            "--broadcast-gold": gold,
            "--broadcast-cyan": cyan,
            opacity: titleCardOpacity(timelineTime, style),
          } as React.CSSProperties}
        >
          <div className="broadcast-title-eyebrow">EPISODE</div>
          <div className="broadcast-title-main">
            {(overlay.episode_title || overlay.show_name).toUpperCase()}
          </div>
          {overlay.episode_subtitle && (
            <div className="broadcast-title-subtitle">
              {overlay.episode_subtitle}
            </div>
          )}
        </div>
      )}

      {inHostIntro ? (
        <div
          className="broadcast-host-intro-strip"
          style={{
            "--broadcast-gold": gold,
            "--broadcast-gold-light": goldLight,
            "--broadcast-navy": navy,
          } as React.CSSProperties}
        >
          <BroadcastIntroHost
            host={overlay.host_a}
            projectRoot={projectRoot}
            resolveAssetUrl={resolveAssetUrl}
          />
          <div className="broadcast-host-intro-divider" />
          <BroadcastIntroHost
            host={overlay.host_b}
            projectRoot={projectRoot}
            resolveAssetUrl={resolveAssetUrl}
            align="right"
          />
        </div>
      ) : (
        <>
          <div
            className="broadcast-name-bar"
            style={{
              "--broadcast-navy": navy,
              "--broadcast-gold": gold,
            } as React.CSSProperties}
          >
            <BroadcastName host={overlay.host_a} />
            <div className="broadcast-name-divider" />
            <BroadcastName host={overlay.host_b} align="right" />
          </div>
          <div
            className="broadcast-ticker"
            style={{
              "--broadcast-navy": navy,
              "--broadcast-gold": gold,
              "--broadcast-cyan": cyan,
            } as React.CSSProperties}
          >
            <div className="broadcast-ticker-show">
              {(overlay.show_name || "BROADCAST").toUpperCase()}
            </div>
            <div className="broadcast-ticker-content">
              <BroadcastSponsorMarquee
                sponsorText={sponsorText}
                timelineTime={timelineTime}
                opacity={tickerPhase.sponsorOpacity}
              />
              {tickerPhase.activeTopic && (
                <div
                  className="broadcast-topic"
                  style={{ opacity: tickerPhase.topicOpacity }}
                >
                  <span>NOW DISCUSSING</span>
                  <strong>{tickerPhase.activeTopic.text}</strong>
                </div>
              )}
            </div>
          </div>
        </>
      )}

      {activeChapter && (
        <div
          className="broadcast-chapter-card"
          style={{
            "--broadcast-navy": navy,
            "--broadcast-gold": gold,
          } as React.CSSProperties}
        >
          <span>{chapterNumber(overlay.chapters, activeChapter)}</span>
          <strong>{activeChapter.text.toUpperCase()}</strong>
        </div>
      )}
        </>
      )}
    </div>
  );
}

function BroadcastAssetPreloads({
  overlay,
  projectRoot,
  resolveAssetUrl,
}: {
  overlay: BroadcastOverlayConfig;
  projectRoot: string | null;
  resolveAssetUrl: (projectRoot: string | null, relPath: string | null) => string | null;
}) {
  const urls = [
    resolveAssetUrl(projectRoot, overlay.brand_logo_path),
    resolveAssetUrl(projectRoot, overlay.host_a.photo_path),
    resolveAssetUrl(projectRoot, overlay.host_b.photo_path),
  ].filter((url): url is string => Boolean(url));
  if (urls.length === 0) return null;
  return (
    <div className="broadcast-asset-preloads">
      {urls.map((url) => (
        <img key={url} src={url} alt="" />
      ))}
    </div>
  );
}

function BroadcastSponsorMarquee({
  sponsorText,
  timelineTime,
  opacity,
}: {
  sponsorText: string;
  timelineTime: number;
  opacity: number;
}) {
  const segmentRef = useRef<HTMLSpanElement | null>(null);
  const [segmentWidth, setSegmentWidth] = useState(0);

  useLayoutEffect(() => {
    const element = segmentRef.current;
    if (!element) return;
    const measure = () => setSegmentWidth(element.getBoundingClientRect().width);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [sponsorText]);

  const scrollPxPerSecond = 48 * (segmentWidth > 0 ? segmentWidth / sponsorTextReferenceWidth(sponsorText) : 0);
  const offset =
    segmentWidth > 0 ? (timelineTime * scrollPxPerSecond) % segmentWidth : 0;

  return (
    <div className="broadcast-sponsor-marquee" style={{ opacity }}>
      <div
        className="broadcast-sponsor-track"
        style={{ transform: `translate3d(${-offset}px, 0, 0)` }}
      >
        <span ref={segmentRef} className="broadcast-sponsor-segment">
          {sponsorText}
        </span>
        <span className="broadcast-sponsor-segment">{sponsorText}</span>
        <span className="broadcast-sponsor-segment">{sponsorText}</span>
      </div>
    </div>
  );
}

function sponsorTextReferenceWidth(text: string): number {
  return Math.max(1, text.length * 28);
}

function BroadcastBrandLogo({
  logoPath,
  projectRoot,
  resolveAssetUrl,
}: {
  logoPath: string | null;
  projectRoot: string | null;
  resolveAssetUrl: (projectRoot: string | null, relPath: string | null) => string | null;
}) {
  const logo = resolveAssetUrl(projectRoot, logoPath);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
  }, [logo]);
  if (!logo || failed) return null;
  return <img src={logo} alt="" onError={() => setFailed(true)} />;
}

function BroadcastName({
  host,
  align,
}: {
  host: BroadcastHost;
  align?: "right";
}) {
  if (!host.name.trim()) return <div />;
  return (
    <div className={`broadcast-name ${align === "right" ? "align-right" : ""}`}>
      <strong>{host.name.toUpperCase()}</strong>
      {host.title && <span>{host.title.toUpperCase()}</span>}
    </div>
  );
}

function BroadcastIntroHost({
  host,
  projectRoot,
  resolveAssetUrl,
  align,
}: {
  host: BroadcastHost;
  projectRoot: string | null;
  resolveAssetUrl: (projectRoot: string | null, relPath: string | null) => string | null;
  align?: "right";
}) {
  const photo = resolveAssetUrl(projectRoot, host.photo_path);
  const [photoFailed, setPhotoFailed] = useState(false);
  useEffect(() => {
    setPhotoFailed(false);
  }, [photo]);
  if (!host.name.trim()) return <div />;
  const initials = host.name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase())
    .join("");
  return (
    <div className={`broadcast-intro-host ${align === "right" ? "align-right" : ""}`}>
      <div className="broadcast-host-photo">
        {photo && !photoFailed ? (
          <img src={photo} alt="" onError={() => setPhotoFailed(true)} />
        ) : (
          <span>{initials}</span>
        )}
      </div>
      <div>
        <strong>{host.name.toUpperCase()}</strong>
        {host.title && <span>{host.title.toUpperCase()}</span>}
      </div>
    </div>
  );
}

function activeChapterEntry(
  entries: BroadcastTimedEntry[],
  timelineTime: number,
  duration: number,
  minStart: number,
): BroadcastTimedEntry | null {
  let active: BroadcastTimedEntry | null = null;
  for (const entry of entries) {
    const start = Math.max(minStart, entry.time_seconds);
    const end = start + Math.max(0.25, duration);
    if (timelineTime >= start && timelineTime < end) active = entry;
  }
  return active;
}

function broadcastTickerEntries(
  overlay: BroadcastOverlayConfig,
): BroadcastTimedEntry[] {
  return overlay.topics.length > 0 ? overlay.topics : overlay.chapters;
}

function broadcastTickerPhase(
  entries: BroadcastTimedEntry[],
  timelineTime: number,
  style: BroadcastOverlayConfig["style"],
): {
  activeTopic: BroadcastTimedEntry | null;
  sponsorOpacity: number;
  topicOpacity: number;
} {
  const topic = [...entries].reverse().find((entry) => entry.time_seconds <= timelineTime);
  if (!topic) {
    return { activeTopic: null, sponsorOpacity: 1, topicOpacity: 0 };
  }
  const sponsor = Math.max(0, style.ticker_sponsor_duration);
  const fade = Math.max(0, style.ticker_fade_duration);
  const topicDuration = Math.max(0.25, style.ticker_topic_duration);
  const cycle = sponsor + fade + topicDuration + fade;
  if (cycle <= 0) return { activeTopic: topic, sponsorOpacity: 0, topicOpacity: 1 };
  const cyclePos = ((timelineTime % cycle) + cycle) % cycle;
  if (cyclePos < sponsor - fade) {
    return { activeTopic: topic, sponsorOpacity: 1, topicOpacity: 0 };
  }
  if (cyclePos < sponsor) {
    const topicOpacity = fade <= 0 ? 1 : (cyclePos - (sponsor - fade)) / fade;
    return {
      activeTopic: topic,
      sponsorOpacity: 1 - topicOpacity,
      topicOpacity,
    };
  }
  if (cyclePos < sponsor + topicDuration) {
    return { activeTopic: topic, sponsorOpacity: 0, topicOpacity: 1 };
  }
  if (cyclePos < sponsor + topicDuration + fade) {
    const sponsorOpacity = fade <= 0 ? 1 : (cyclePos - sponsor - topicDuration) / fade;
    return {
      activeTopic: topic,
      sponsorOpacity,
      topicOpacity: 1 - sponsorOpacity,
    };
  }
  return { activeTopic: topic, sponsorOpacity: 1, topicOpacity: 0 };
}

function chapterNumber(
  chapters: BroadcastTimedEntry[],
  active: BroadcastTimedEntry,
): string {
  const index = chapters.findIndex((chapter) => chapter === active);
  return String(index >= 0 ? index + 1 : 1);
}

function titleCardOpacity(
  t: number,
  style: BroadcastOverlayConfig["style"],
): number {
  const fadeIn = Math.max(0.001, style.title_fade_in_end);
  const fadeOutStart = style.title_fade_out_start;
  const end = Math.max(fadeOutStart + 0.001, style.title_visible_end);
  if (t < fadeIn) return Math.max(0, Math.min(1, t / fadeIn));
  if (t < fadeOutStart) return 1;
  return Math.max(0, Math.min(1, (end - t) / (end - fadeOutStart)));
}

function normalizeCssHex(value: string, fallback: string): string {
  if (!value.trim()) return fallback;
  return value.startsWith("#") ? value : `#${value}`;
}

function responsiveBroadcastOverlayScale(
  previewFrameSize: { width: number; height: number } | undefined,
): number {
  if (!previewFrameSize || previewFrameSize.width <= 0 || previewFrameSize.height <= 0) {
    return 1;
  }
  const widthScale = previewFrameSize.width / 960;
  const heightScale = previewFrameSize.height / 540;
  return Math.max(0.62, Math.min(1, widthScale, heightScale));
}

function refHeightPercent(value: number): string {
  return `${(value / 2160) * 100}%`;
}

function refWidthPercent(value: number): string {
  return `${(value / 3840) * 100}%`;
}
