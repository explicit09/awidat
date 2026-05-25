//! Typed capability metadata for tools, effects, and render features.
//!
//! The goal is to state support honestly in machine-readable fields instead
//! of hiding "stub" or "partial" status inside prose descriptions.

/// Three-state support marker. Use `Unknown` when support has not been
/// verified instead of overclaiming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportLevel {
    /// The feature is expected to work.
    Supported,
    /// The feature is known not to be supported.
    NotSupported,
    /// Support is not known from current evidence.
    Unknown,
}

/// Cross-surface metadata shared by tools, effects, and render features.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CapabilityMetadata {
    /// Whether invoking/applying this capability mutates the project graph.
    pub graph_mutates: bool,
    /// Whether the desktop preview path is known to support this capability.
    pub preview_supported: SupportLevel,
    /// Whether final export/render is known to support this capability.
    pub export_supported: SupportLevel,
    /// Indexes that should exist before this capability is useful.
    pub required_indexes: Vec<String>,
    /// Whether a user approval gate is required by default.
    pub approval_required: bool,
    /// Non-graph side effects, such as network access or files written.
    pub side_effects: Vec<String>,
    /// Known gaps or partial-support caveats.
    pub known_limitations: Vec<String>,
}

impl CapabilityMetadata {
    /// Conservative default for tools without an explicit entry.
    pub fn tool_default(graph_mutates: bool) -> Self {
        Self {
            graph_mutates,
            preview_supported: SupportLevel::Unknown,
            export_supported: SupportLevel::Unknown,
            required_indexes: Vec::new(),
            approval_required: graph_mutates,
            side_effects: if graph_mutates {
                vec!["may mutate the project graph or filesystem".into()]
            } else {
                Vec::new()
            },
            known_limitations: Vec::new(),
        }
    }

    /// Metadata for named built-in tools. Unknown names keep the
    /// conservative default supplied by the caller.
    pub fn for_tool_name(name: &str, graph_mutates: bool) -> Self {
        let mut metadata = Self::tool_default(graph_mutates);
        match name {
            "find_moment" => {
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.required_indexes = vec!["editorial_moments".into()];
            }
            "clip_search" => {
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.required_indexes = vec!["clip_embeddings".into()];
            }
            "transcript_pack"
            | "transcript_search"
            | "find_dead_air"
            | "find_filler_words"
            | "find_false_starts"
            | "find_episode_start"
            | "podcast_editorial_review_pack"
            | "find_generated_broll_opportunities" => {
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.required_indexes = vec!["transcript".into()];
            }
            "find_eye_contact" | "find_speaker_oncam" | "shot_summary" => {
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.required_indexes = vec!["shot".into()];
            }
            "find_beat" => {
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.required_indexes = vec!["beats".into()];
            }
            "color_scopes" | "list_looks" | "plan_look_regions" => {
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.required_indexes = vec!["color_analysis".into()];
            }
            "start_render" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::NotSupported;
                metadata.export_supported = SupportLevel::Supported;
                metadata.approval_required = true;
                metadata.side_effects = vec![
                    "starts an ffmpeg render job".into(),
                    "writes render output files".into(),
                ];
            }
            "poll_render" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::NotSupported;
                metadata.export_supported = SupportLevel::Supported;
                metadata.approval_required = false;
                metadata.side_effects = Vec::new();
            }
            "render_preflight" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::NotSupported;
                metadata.export_supported = SupportLevel::Supported;
                metadata.approval_required = false;
                metadata.side_effects = vec![
                    "reports backend selection and capability metadata with no render job started"
                        .into(),
                ];
                metadata.known_limitations = vec![
                    "currently preflights edited timeline renders".into(),
                    "optional preview-cache planning is read-only and does not generate artifacts"
                        .into(),
                ];
            }
            "verify_render" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::NotSupported;
                metadata.export_supported = SupportLevel::Supported;
                metadata.approval_required = true;
                metadata.side_effects = vec![
                    "writes render verification reports".into(),
                    "updates render manifest verification summaries".into(),
                ];
                metadata.known_limitations = vec![
                    "caption rendered-output evidence is now produced by the frame-pixel scorer when ffmpeg and a render output are available; libass-layout sidecar derivation remains a named fallback when the scorer is unavailable"
                        .into(),
                ];
            }
            "export_package" => {
                metadata.preview_supported = SupportLevel::NotSupported;
                metadata.export_supported = SupportLevel::Supported;
                metadata.side_effects = vec![
                    "starts a render job".into(),
                    "writes delivery package files".into(),
                ];
            }
            "stream_remux" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::NotSupported;
                metadata.export_supported = SupportLevel::Supported;
                metadata.approval_required = true;
                metadata.side_effects = vec![
                    "starts an ffmpeg stream-copy/remux job".into(),
                    "writes remuxed media output and a render manifest".into(),
                ];
                metadata.known_limitations = vec![
                    "not suitable for frame-domain effects, styled overlays, transitions, retiming, or caption burn-in"
                        .into(),
                ];
            }
            "download_yt_clip" | "search_broll" => {
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.side_effects = vec!["uses network access".into()];
            }
            "plan_generated_media" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::Unknown;
                metadata.approval_required = false;
                metadata.side_effects = Vec::new();
            }
            "poll_generated_media_job" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::Unknown;
                metadata.approval_required = true;
                metadata.side_effects = vec![
                    "reads generated media registry records".into(),
                    "polls OpenRouter jobs and downloads completed outputs under raw/generated"
                        .into(),
                    "updates generated media registry records".into(),
                ];
            }
            "use_generated_media" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::Unknown;
                metadata.approval_required = false;
                metadata.side_effects = vec!["reads generated media registry records".into()];
                metadata.known_limitations =
                    vec!["returns an EDL fragment but does not apply it".into()];
            }
            "start_generated_media_job" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::Unknown;
                metadata.approval_required = true;
                metadata.side_effects = vec![
                    "writes generated media registry records".into(),
                    "openrouter provider submits network video generation jobs when OPENROUTER_API_KEY is set".into(),
                    "mock provider writes placeholder media under raw/generated".into(),
                ];
            }
            "use_broll" | "apply_edl" => {
                metadata.graph_mutates = true;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::Unknown;
                metadata.approval_required = true;
                metadata.side_effects = vec!["writes project timeline changes".into()];
            }
            "proxy_status" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.approval_required = false;
                metadata.side_effects = vec!["reads proxy cache artifact metadata".into()];
            }
            "preview_cache_status" => {
                metadata.graph_mutates = false;
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::NotSupported;
                metadata.approval_required = false;
                metadata.side_effects =
                    vec!["reads proxy, thumbnail, and waveform cache metadata".into()];
                metadata.known_limitations = vec![
                    "preview-cache refresh now runs the persisted lifecycle through `PreviewRefreshExecutor` with resume semantics; cross-process file locking is not yet implemented"
                        .into(),
                ];
            }
            "import_media" | "import_url" | "generate_proxy" | "proxy_media" => {
                metadata.preview_supported = SupportLevel::Supported;
                metadata.export_supported = SupportLevel::Unknown;
                metadata.side_effects = vec!["writes media or proxy files".into()];
            }
            "bash" => {
                metadata.known_limitations =
                    vec!["side effects depend on the command arguments".into()];
            }
            _ => {}
        }
        metadata
    }

    /// Metadata for a graph-native effect.
    pub fn for_effect(
        support: awidat_effects::SupportStatus,
        backend: awidat_effects::BackendKind,
    ) -> Self {
        let export_supported = match (support, backend) {
            (awidat_effects::SupportStatus::Stable, awidat_effects::BackendKind::FfmpegNative)
            | (
                awidat_effects::SupportStatus::Experimental,
                awidat_effects::BackendKind::FfmpegNative,
            ) => SupportLevel::Supported,
            (awidat_effects::SupportStatus::Unavailable, _) => SupportLevel::NotSupported,
            (_, awidat_effects::BackendKind::SemanticOnly) => SupportLevel::NotSupported,
        };
        let mut known_limitations = Vec::new();
        if support == awidat_effects::SupportStatus::Experimental {
            known_limitations.push(
                "experimental effect support; some parameter combinations may emit render limitations"
                    .into(),
            );
        }
        Self {
            graph_mutates: true,
            preview_supported: SupportLevel::Unknown,
            export_supported,
            required_indexes: Vec::new(),
            approval_required: true,
            side_effects: vec!["writes OTIO effect metadata when applied".into()],
            known_limitations,
        }
    }

    /// Metadata for a render/preview pipeline feature.
    pub fn render_feature(
        preview_supported: SupportLevel,
        export_supported: SupportLevel,
        approval_required: bool,
        side_effects: Vec<String>,
        known_limitations: Vec<String>,
    ) -> Self {
        Self {
            graph_mutates: false,
            preview_supported,
            export_supported,
            required_indexes: Vec::new(),
            approval_required,
            side_effects,
            known_limitations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_media_tool_metadata_reports_registry_and_approval_behavior() {
        let find = CapabilityMetadata::for_tool_name("find_generated_broll_opportunities", false);
        assert!(!find.approval_required);
        assert_eq!(find.required_indexes, vec!["transcript"]);

        let start = CapabilityMetadata::for_tool_name("start_generated_media_job", true);
        assert!(!start.graph_mutates);
        assert!(start.approval_required);
        assert!(
            start
                .side_effects
                .iter()
                .any(|effect| effect.contains("registry records"))
        );

        let poll = CapabilityMetadata::for_tool_name("poll_generated_media_job", false);
        assert!(poll.approval_required);
        assert!(
            poll.side_effects
                .iter()
                .any(|effect| effect.contains("downloads completed outputs"))
        );

        let use_generated = CapabilityMetadata::for_tool_name("use_generated_media", false);
        assert!(!use_generated.graph_mutates);
        assert!(
            use_generated
                .known_limitations
                .iter()
                .any(|limitation| limitation.contains("does not apply it"))
        );
    }
}
