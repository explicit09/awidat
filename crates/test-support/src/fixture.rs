//! Filesystem fixtures: temp dirs and initialized awidat projects.

use awidat_proto::project::Project;
use tempfile::TempDir;

/// Unique per-test temp directory. The returned [`TempDir`] cleans on drop —
/// hold the binding for the duration of the test.
pub fn tmp_dir() -> TempDir {
    tempfile::tempdir().expect("tempdir creation failed; check disk")
}

/// Initialize a fresh awidat project at a unique temp path. Returns the
/// in-memory [`Project`] alongside the [`TempDir`] guard. Drop the guard
/// (or let it go out of scope) to remove the project from disk.
///
/// ```
/// # use awidat_test_support::fixture;
/// let (project, _guard) = fixture::project();
/// assert_eq!(project.timeline.otio_schema, "Timeline.1");
/// ```
pub fn project() -> (Project, TempDir) {
    let dir = tmp_dir();
    let project = Project::init(dir.path()).expect("Project::init in fresh tempdir must succeed");
    (project, dir)
}
