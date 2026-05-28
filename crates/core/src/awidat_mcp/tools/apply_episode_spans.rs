//! `apply_episode_spans` — persist reviewed episode spans into OTIO metadata.

use std::collections::HashSet;

use awidat_proto::awidat_meta::{EpisodeSpan, EpisodeSpanStatus};
use awidat_proto::professional::{SelectDecision, SourceRange, SourceSelect, Stringout};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::media_catalog_mutation::ensure_awidat_metadata;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ApplyEpisodeSpansArgs {
    #[serde(default)]
    pub episodes: Vec<EpisodeSpanInput>,
    /// Replace all stored episodes when true; otherwise upsert by id.
    #[serde(default)]
    pub replace: bool,
    /// Create/update source selects and an ordered stringout for accepted episodes.
    #[serde(default)]
    pub create_stringouts: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct EpisodeSpanInput {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub order: Option<u32>,
    pub asset_id: String,
    pub source_start_s: f64,
    pub source_end_s: f64,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default = "default_episode_status")]
    pub status: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ApplyEpisodeSpansResponse {
    status: &'static str,
    replace: bool,
    create_stringouts: bool,
    applied: usize,
    total: usize,
    accepted_stringout_items: usize,
}

fn default_episode_status() -> String {
    "review_needed".to_string()
}

pub fn run(args: ApplyEpisodeSpansArgs, ctx: McpToolCtx) -> Result<String, String> {
    let replace = args.replace;
    let create_stringouts = args.create_stringouts;
    let applied = args.episodes.len();
    let episodes = normalize_episode_inputs(args.episodes)?;

    let mut project = Project::read(&ctx.project_root)
        .map_err(|e| format!("apply_episode_spans: unable to read project: {e}"))?;
    let meta = ensure_awidat_metadata(&mut project.timeline);
    if replace {
        meta.episodes = episodes;
    } else {
        for episode in episodes {
            match meta
                .episodes
                .iter_mut()
                .find(|existing| existing.id == episode.id)
            {
                Some(existing) => *existing = episode,
                None => meta.episodes.push(episode),
            }
        }
    }
    let accepted_stringout_items = if create_stringouts {
        upsert_episode_stringout(meta)
    } else {
        0
    };
    let total = meta.episodes.len();

    project
        .write(&ctx.project_root)
        .map_err(|e| format!("apply_episode_spans: unable to write project: {e}"))?;

    let response = ApplyEpisodeSpansResponse {
        status: "applied",
        replace,
        create_stringouts,
        applied,
        total,
        accepted_stringout_items,
    };
    serde_json::to_string(&response).map_err(|e| format!("apply_episode_spans serialize: {e}"))
}

fn upsert_episode_stringout(meta: &mut awidat_proto::awidat_meta::AwidatTimelineMetadata) -> usize {
    let mut accepted = meta
        .episodes
        .iter()
        .filter(|episode| episode.status == EpisodeSpanStatus::Accepted)
        .cloned()
        .collect::<Vec<_>>();
    accepted.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.source_start_s.total_cmp(&right.source_start_s))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut select_ids = Vec::with_capacity(accepted.len());
    for episode in accepted {
        let select_id = format!("episode:{}:select", episode.id);
        let select = SourceSelect {
            id: select_id.clone(),
            asset_id: episode.asset_id.clone(),
            range: SourceRange {
                start_s: episode.source_start_s,
                end_s: episode.source_end_s,
            },
            decision: SelectDecision::Select,
            reason: Some(format!("accepted episode span {}", episode.id)),
            evidence_refs: episode.evidence.clone(),
            ..SourceSelect::default()
        };
        match meta
            .selects
            .iter_mut()
            .find(|existing| existing.id == select_id)
        {
            Some(existing) => *existing = select,
            None => meta.selects.push(select),
        }
        select_ids.push(select_id);
    }

    let kept: HashSet<&str> = select_ids.iter().map(String::as_str).collect();
    meta.selects
        .retain(|select| !is_episode_select(&select.id) || kept.contains(select.id.as_str()));

    let item_count = select_ids.len();
    let stringout = Stringout {
        id: "episodes-accepted".into(),
        name: Some("Accepted episodes".into()),
        select_ids,
    };
    match meta
        .stringouts
        .iter_mut()
        .find(|existing| existing.id == stringout.id)
    {
        Some(existing) => *existing = stringout,
        None => meta.stringouts.push(stringout),
    }
    item_count
}

fn is_episode_select(id: &str) -> bool {
    id.starts_with("episode:") && id.ends_with(":select")
}

fn normalize_episode_inputs(inputs: Vec<EpisodeSpanInput>) -> Result<Vec<EpisodeSpan>, String> {
    let mut ids = HashSet::new();
    let mut episodes = Vec::with_capacity(inputs.len());
    for input in inputs {
        let id = input.id.trim().to_string();
        if id.is_empty() {
            return Err("apply_episode_spans: episode id must not be empty".into());
        }
        if !ids.insert(id.clone()) {
            return Err(format!(
                "apply_episode_spans: duplicate episode id {id} in request"
            ));
        }
        let asset_id = input.asset_id.trim().to_string();
        if asset_id.is_empty() {
            return Err(format!(
                "apply_episode_spans: episode {id} asset_id must not be empty"
            ));
        }
        if !input.source_start_s.is_finite()
            || !input.source_end_s.is_finite()
            || input.source_end_s <= input.source_start_s
        {
            return Err(format!(
                "apply_episode_spans: episode {id} source_start_s {} must be before source_end_s {}",
                input.source_start_s, input.source_end_s
            ));
        }
        if let Some(confidence) = input.confidence
            && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err(format!(
                "apply_episode_spans: episode {id} confidence {confidence} must be between 0 and 1"
            ));
        }
        let status = parse_status(&input.status)
            .map_err(|e| format!("apply_episode_spans: episode {id} {e}"))?;
        episodes.push(EpisodeSpan {
            id,
            name: input
                .name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty()),
            order: input.order,
            asset_id,
            source_start_s: input.source_start_s,
            source_end_s: input.source_end_s,
            confidence: input.confidence,
            status,
            evidence: input.evidence,
            ..EpisodeSpan::default()
        });
    }
    Ok(episodes)
}

fn parse_status(status: &str) -> Result<EpisodeSpanStatus, String> {
    match status.trim() {
        "" | "review_needed" => Ok(EpisodeSpanStatus::ReviewNeeded),
        "accepted" => Ok(EpisodeSpanStatus::Accepted),
        "rejected" => Ok(EpisodeSpanStatus::Rejected),
        other => Err(format!(
            "status must be one of review_needed, accepted, rejected; got {other}"
        )),
    }
}

pub const DESCRIPTION: &str = "\
Persist reviewed episode spans into Timeline.metadata.awidat.episodes. \
Use this after podcast_episode_spans or transcript review to make episodes \
first-class project metadata. With replace=true, replaces all stored \
episodes; with replace=false, upserts by id. Each episode requires id, \
asset_id, source_start_s, source_end_s, and status one of review_needed, \
accepted, or rejected. Set create_stringouts=true to create/update source \
selects and an ordered stringout for accepted episodes.";

#[cfg(test)]
mod tests {
    use awidat_proto::project::Project;

    use super::*;
    use crate::awidat_mcp::tools::list_episodes::{self, ListEpisodesArgs};

    fn ctx_at(path: &std::path::Path) -> McpToolCtx {
        McpToolCtx {
            project_root: path.to_path_buf(),
        }
    }

    fn episode(id: &str, status: &str, start: f64, end: f64) -> EpisodeSpanInput {
        EpisodeSpanInput {
            id: id.into(),
            name: Some(id.replace('-', " ")),
            order: Some(1),
            asset_id: "raw/interview.mov".into(),
            source_start_s: start,
            source_end_s: end,
            confidence: Some(0.8),
            status: status.into(),
            evidence: vec!["intro_language".into()],
        }
    }

    #[test]
    fn episode_tools_apply_and_list_spans() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        let ctx = ctx_at(dir.path());

        let applied = run(
            ApplyEpisodeSpansArgs {
                episodes: vec![
                    episode("episode-1", "accepted", 10.0, 110.0),
                    episode("episode-2", "review_needed", 140.0, 240.0),
                ],
                replace: true,
                create_stringouts: false,
            },
            ctx.clone(),
        )
        .unwrap();
        assert!(applied.contains("\"applied\":2"));

        let listed = list_episodes::run(ListEpisodesArgs {}, ctx).unwrap();
        let value: serde_json::Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(value["total"], 2);
        assert_eq!(value["episodes"][0]["id"], "episode-1");
        assert_eq!(value["episodes"][0]["duration_s"], 100.0);
        assert_eq!(value["episodes"][0]["status"], "accepted");
        assert_eq!(value["episodes"][1]["status"], "review_needed");
    }

    #[test]
    fn episode_tools_upsert_by_id_when_replace_is_false() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        let ctx = ctx_at(dir.path());

        run(
            ApplyEpisodeSpansArgs {
                episodes: vec![episode("episode-1", "review_needed", 10.0, 110.0)],
                replace: true,
                create_stringouts: false,
            },
            ctx.clone(),
        )
        .unwrap();
        run(
            ApplyEpisodeSpansArgs {
                episodes: vec![episode("episode-1", "accepted", 12.0, 112.0)],
                replace: false,
                create_stringouts: false,
            },
            ctx.clone(),
        )
        .unwrap();

        let listed = list_episodes::run(ListEpisodesArgs {}, ctx).unwrap();
        let value: serde_json::Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(value["total"], 1);
        assert_eq!(value["episodes"][0]["source_start_s"], 12.0);
        assert_eq!(value["episodes"][0]["status"], "accepted");
    }

    #[test]
    fn episode_tools_reject_invalid_ranges() {
        let err = normalize_episode_inputs(vec![episode("episode-1", "accepted", 20.0, 10.0)])
            .unwrap_err();
        assert!(err.contains("source_start_s 20 must be before source_end_s 10"));
    }

    #[test]
    fn episode_tools_create_stringout_for_accepted_spans_only() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        let ctx = ctx_at(dir.path());

        let mut second = episode("episode-2", "accepted", 200.0, 300.0);
        second.order = Some(2);
        let mut first = episode("episode-1", "accepted", 10.0, 110.0);
        first.order = Some(1);
        let rejected = episode("false-start", "rejected", 120.0, 150.0);

        let applied = run(
            ApplyEpisodeSpansArgs {
                episodes: vec![second, first, rejected],
                replace: true,
                create_stringouts: true,
            },
            ctx,
        )
        .unwrap();
        assert!(applied.contains("\"accepted_stringout_items\":2"));

        let project = Project::read(dir.path()).unwrap();
        let meta = project.timeline.metadata.awidat.unwrap();
        let stringout = meta
            .stringouts
            .iter()
            .find(|stringout| stringout.id == "episodes-accepted")
            .unwrap();
        assert_eq!(
            stringout.select_ids,
            vec!["episode:episode-1:select", "episode:episode-2:select"]
        );
        assert_eq!(meta.selects.len(), 2);
        assert!(
            meta.selects
                .iter()
                .all(|select| select.decision == SelectDecision::Select)
        );
        assert!(
            !meta
                .selects
                .iter()
                .any(|select| select.id.contains("false-start"))
        );
    }

    #[test]
    fn episode_tools_prune_stale_selects_on_regenerate() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        let ctx = ctx_at(dir.path());

        run(
            ApplyEpisodeSpansArgs {
                episodes: vec![
                    episode("episode-1", "accepted", 10.0, 110.0),
                    episode("episode-2", "accepted", 200.0, 300.0),
                ],
                replace: true,
                create_stringouts: true,
            },
            ctx.clone(),
        )
        .unwrap();

        run(
            ApplyEpisodeSpansArgs {
                episodes: vec![episode("episode-1", "accepted", 10.0, 110.0)],
                replace: true,
                create_stringouts: true,
            },
            ctx.clone(),
        )
        .unwrap();

        let project = Project::read(dir.path()).unwrap();
        let meta = project.timeline.metadata.awidat.unwrap();
        let episode_select_ids: Vec<_> = meta
            .selects
            .iter()
            .filter(|select| is_episode_select(&select.id))
            .map(|select| select.id.clone())
            .collect();
        assert_eq!(episode_select_ids, vec!["episode:episode-1:select"]);
    }
}
