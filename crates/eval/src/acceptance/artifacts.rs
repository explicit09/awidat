use std::fs;
use std::path::Path;

use chrono::Utc;
use serde::Serialize;

pub(crate) fn build_artifact_bundle_manifest(
    input: ArtifactBundleInput<'_>,
) -> ArtifactBundleManifest {
    let entries = artifact_entries(vec![
        ("rendered_output", input.output_path, true),
        ("render_manifest", input.render_manifest_path, true),
        ("final_edl", input.final_edl_path, true),
        ("edit_manifest", input.edit_manifest_path, true),
        ("ffprobe_json", input.ffprobe_json_path, true),
        ("silence_json", input.silence_path, true),
        ("blackdetect_json", input.blackdetect_path, true),
        ("scorecard_json", input.scorecard_path, true),
    ])
    .into_iter()
    .chain(
        input
            .transcript_path
            .map(|path| artifact_entries(vec![("transcript_json", path, true)]))
            .unwrap_or_default(),
    )
    .chain(optional_generator_entries(
        input.generator_stdout_path,
        input.generator_stderr_path,
    ))
    .collect::<Vec<_>>();
    ArtifactBundleManifest {
        generated_at: Utc::now().to_rfc3339(),
        run_root: input.run_root.to_string_lossy().into_owned(),
        complete: entries.iter().all(|entry| {
            !entry.required || (entry.exists && entry.size_bytes.unwrap_or_default() > 0)
        }),
        entries,
    }
}

pub(crate) fn required_artifact_entries(
    input: RequiredArtifactInput<'_>,
) -> Vec<ArtifactBundleEntry> {
    let mut entries = artifact_entries(vec![
        ("rendered_output", input.output_path, true),
        ("render_manifest", input.render_manifest_path, true),
        ("final_edl", input.final_edl_path, true),
        ("edit_manifest", input.edit_manifest_path, true),
        ("ffprobe_json", input.ffprobe_json_path, true),
        ("silence_json", input.silence_path, true),
        ("blackdetect_json", input.blackdetect_path, true),
    ]);
    if let Some(transcript_path) = input.transcript_path {
        entries.extend(artifact_entries(vec![(
            "transcript_json",
            transcript_path,
            true,
        )]));
    }
    entries.extend(optional_generator_entries(
        input.generator_stdout_path,
        input.generator_stderr_path,
    ));
    entries
}

pub(crate) fn required_artifacts_present(entries: &[ArtifactBundleEntry]) -> bool {
    entries
        .iter()
        .all(|entry| !entry.required || (entry.exists && entry.size_bytes.unwrap_or_default() > 0))
}

fn optional_generator_entries(
    stdout_path: Option<&Path>,
    stderr_path: Option<&Path>,
) -> Vec<ArtifactBundleEntry> {
    let mut entries = Vec::new();
    if let Some(path) = stdout_path {
        entries.extend(artifact_entries(vec![("edl_generator_stdout", path, true)]));
    }
    if let Some(path) = stderr_path {
        entries.extend(artifact_entries(vec![(
            "edl_generator_stderr",
            path,
            false,
        )]));
    }
    entries
}

fn artifact_entries(entries: Vec<(&str, &Path, bool)>) -> Vec<ArtifactBundleEntry> {
    entries
        .into_iter()
        .map(|(name, path, required)| {
            let metadata = fs::metadata(path).ok();
            ArtifactBundleEntry {
                name: name.into(),
                path: path.to_string_lossy().into_owned(),
                required,
                exists: metadata.is_some(),
                size_bytes: metadata.map(|metadata| metadata.len()),
            }
        })
        .collect()
}
#[derive(Debug, Serialize)]
pub(crate) struct ArtifactBundleManifest {
    pub(crate) generated_at: String,
    pub(crate) run_root: String,
    pub(crate) complete: bool,
    pub(crate) entries: Vec<ArtifactBundleEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ArtifactBundleEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) required: bool,
    pub(crate) exists: bool,
    pub(crate) size_bytes: Option<u64>,
}

pub(crate) struct RequiredArtifactInput<'a> {
    pub(crate) output_path: &'a Path,
    pub(crate) render_manifest_path: &'a Path,
    pub(crate) final_edl_path: &'a Path,
    pub(crate) edit_manifest_path: &'a Path,
    pub(crate) ffprobe_json_path: &'a Path,
    pub(crate) silence_path: &'a Path,
    pub(crate) blackdetect_path: &'a Path,
    pub(crate) transcript_path: Option<&'a Path>,
    pub(crate) generator_stdout_path: Option<&'a Path>,
    pub(crate) generator_stderr_path: Option<&'a Path>,
}

pub(crate) struct ArtifactBundleInput<'a> {
    pub(crate) run_root: &'a Path,
    pub(crate) output_path: &'a Path,
    pub(crate) render_manifest_path: &'a Path,
    pub(crate) final_edl_path: &'a Path,
    pub(crate) edit_manifest_path: &'a Path,
    pub(crate) ffprobe_json_path: &'a Path,
    pub(crate) silence_path: &'a Path,
    pub(crate) blackdetect_path: &'a Path,
    pub(crate) transcript_path: Option<&'a Path>,
    pub(crate) generator_stdout_path: Option<&'a Path>,
    pub(crate) generator_stderr_path: Option<&'a Path>,
    pub(crate) scorecard_path: &'a Path,
}
