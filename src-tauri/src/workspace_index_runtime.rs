use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::{
    commands::open_regular_file_without_following_links,
    workspace_file_kind::WorkspaceFileKind,
    workspace_index::{CancellationToken, WorkspaceIndex},
};

const MAX_ACTIVE_OPERATIONS: usize = 16;
const MAX_OPERATION_DURATION: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceIndexScope {
    workspace_token: String,
    workspace_root: PathBuf,
    generation: u64,
}

struct StoredWorkspaceIndex {
    index: Arc<WorkspaceIndex>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind {
    Rebuild,
    Query,
}

struct ActiveOperation {
    scope: WorkspaceIndexScope,
    kind: OperationKind,
    cancellation: CancellationToken,
}

struct ActiveWorkspaceIndex {
    scope: WorkspaceIndexScope,
    index: Option<StoredWorkspaceIndex>,
}

trait WatchHandlePort: Send {
    fn stop(&mut self);
}

struct NativeWatchHandle {
    watcher: Option<RecommendedWatcher>,
}

impl WatchHandlePort for NativeWatchHandle {
    fn stop(&mut self) {
        self.watcher.take();
    }
}

impl Drop for NativeWatchHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct RuntimeState {
    active: Option<ActiveWorkspaceIndex>,
    operations: std::collections::HashMap<String, ActiveOperation>,
    watcher: Option<Box<dyn WatchHandlePort>>,
    next_generation: u64,
}

/// A generation-bound operation lease. A completion must be discarded unless
/// the same scope and generation remain current when it is published.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceIndexLease {
    pub(crate) workspace_token: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) generation: u64,
    pub(crate) cancellation: CancellationToken,
    deadline: Instant,
}

impl WorkspaceIndexLease {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled() || Instant::now() >= self.deadline
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceIndexQueryLease {
    pub(crate) lease: WorkspaceIndexLease,
    pub(crate) index: Arc<WorkspaceIndex>,
}

pub(crate) struct WorkspaceIndexRuntime {
    state: Arc<Mutex<RuntimeState>>,
}

impl Default for WorkspaceIndexRuntime {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(RuntimeState::default())),
        }
    }
}

impl WorkspaceIndexRuntime {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RuntimeState>, String> {
        self.state
            .lock()
            .map_err(|_| "Workspace index state is unavailable".to_string())
    }

    pub(crate) fn begin_rebuild(
        &self,
        workspace_token: &str,
        workspace_root: &Path,
        operation_id: &str,
    ) -> Result<WorkspaceIndexLease, String> {
        validate_operation_id(operation_id)?;
        let (scope, cancellation, deadline, previous_watcher) = {
            let mut state = self.lock()?;
            if state.operations.contains_key(operation_id) {
                return Err("Workspace index operation is already active".to_string());
            }
            if state.operations.len() >= MAX_ACTIVE_OPERATIONS {
                return Err("Too many workspace index operations are active".to_string());
            }

            cancel_operations(&mut state.operations, None);
            let generation = allocate_generation(&mut state, 0)?;
            let scope = WorkspaceIndexScope {
                workspace_token: workspace_token.to_string(),
                workspace_root: workspace_root.to_path_buf(),
                generation,
            };
            let previous_watcher = state.watcher.take();
            state.active = Some(ActiveWorkspaceIndex {
                scope: scope.clone(),
                index: None,
            });
            let (cancellation, deadline) = new_deadline_bound_cancellation();
            state.operations.insert(
                operation_id.to_string(),
                ActiveOperation {
                    scope: scope.clone(),
                    kind: OperationKind::Rebuild,
                    cancellation: cancellation.clone(),
                },
            );
            (scope, cancellation, deadline, previous_watcher)
        };
        stop_watch(previous_watcher);

        let watcher =
            match create_native_watch(workspace_root, Arc::downgrade(&self.state), scope.clone()) {
                Ok(watcher) => watcher,
                Err(error) => {
                    invalidate_exact_binding(&self.state, &scope, true);
                    self.end_operation(operation_id);
                    return Err(error);
                }
            };
        let mut state = self.lock()?;
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.scope == scope)
        {
            state.watcher = Some(Box::new(watcher));
        }
        drop(state);
        Ok(WorkspaceIndexLease {
            workspace_token: scope.workspace_token,
            workspace_root: scope.workspace_root,
            generation: scope.generation,
            cancellation,
            deadline,
        })
    }

    pub(crate) fn begin_query(
        &self,
        workspace_token: &str,
        workspace_root: &Path,
        operation_id: &str,
    ) -> Result<WorkspaceIndexQueryLease, String> {
        validate_operation_id(operation_id)?;
        let mut state = self.lock()?;
        if state.operations.contains_key(operation_id) {
            return Err("Workspace index operation is already active".to_string());
        }
        if state.operations.len() >= MAX_ACTIVE_OPERATIONS {
            return Err("Too many workspace index operations are active".to_string());
        }
        let active = state
            .active
            .as_ref()
            .filter(|active| scope_matches(&active.scope, workspace_token, workspace_root))
            .ok_or_else(|| "Workspace index does not match the selected workspace".to_string())?;
        let stored = active
            .index
            .as_ref()
            .ok_or_else(|| "Workspace index has not been built".to_string())?;
        let scope = active.scope.clone();
        let index = Arc::clone(&stored.index);

        // Search is latest-wins within one workspace; a newer keystroke must not
        // leave an older response eligible to update the dialog.
        cancel_operations(&mut state.operations, Some((&scope, OperationKind::Query)));
        let (cancellation, deadline) = new_deadline_bound_cancellation();
        state.operations.insert(
            operation_id.to_string(),
            ActiveOperation {
                scope: scope.clone(),
                kind: OperationKind::Query,
                cancellation: cancellation.clone(),
            },
        );
        Ok(WorkspaceIndexQueryLease {
            lease: WorkspaceIndexLease {
                workspace_token: scope.workspace_token,
                workspace_root: scope.workspace_root,
                generation: scope.generation,
                cancellation,
                deadline,
            },
            index,
        })
    }

    pub(crate) fn publish_rebuild(
        &self,
        lease: &WorkspaceIndexLease,
        index: WorkspaceIndex,
    ) -> Result<bool, String> {
        let mut state = self.lock()?;
        let Some(active) = state.active.as_mut() else {
            return Ok(false);
        };
        if !lease_matches(&active.scope, lease) || lease.is_cancelled() {
            return Ok(false);
        }
        active.index = Some(StoredWorkspaceIndex {
            index: Arc::new(index),
        });
        Ok(true)
    }

    pub(crate) fn is_current(&self, lease: &WorkspaceIndexLease) -> Result<bool, String> {
        let state = self.lock()?;
        Ok(state.active.as_ref().is_some_and(|active| {
            active.index.is_some() && lease_matches(&active.scope, lease) && !lease.is_cancelled()
        }))
    }

    pub(crate) fn current_generation(
        &self,
        workspace_token: &str,
        workspace_root: &Path,
    ) -> Result<Option<u64>, String> {
        let state = self.lock()?;
        Ok(state
            .active
            .as_ref()
            .filter(|active| scope_matches(&active.scope, workspace_token, workspace_root))
            .map(|active| active.scope.generation))
    }

    pub(crate) fn is_result_current(
        &self,
        workspace_token: &str,
        workspace_root: &Path,
        generation: u64,
    ) -> Result<bool, String> {
        let state = self.lock()?;
        Ok(state.active.as_ref().is_some_and(|active| {
            active.index.is_some()
                && active.scope.generation == generation
                && scope_matches(&active.scope, workspace_token, workspace_root)
        }))
    }

    pub(crate) fn invalidate(
        &self,
        workspace_token: &str,
        workspace_root: &Path,
    ) -> Result<bool, String> {
        let watcher = {
            let mut state = self.lock()?;
            let Some(active) = state
                .active
                .as_ref()
                .filter(|active| scope_matches(&active.scope, workspace_token, workspace_root))
            else {
                return Ok(false);
            };
            let scope = active.scope.clone();
            invalidate_locked(&mut state, &scope)?;
            state.watcher.take()
        };
        stop_watch(watcher);
        Ok(true)
    }

    pub(crate) fn discard(
        &self,
        workspace_token: &str,
        workspace_root: &Path,
    ) -> Result<bool, String> {
        self.invalidate(workspace_token, workspace_root)
    }

    pub(crate) fn discard_all(&self) {
        let watcher = {
            let Ok(mut state) = self.lock() else {
                return;
            };
            cancel_operations(&mut state.operations, None);
            state.active = None;
            state.watcher.take()
        };
        stop_watch(watcher);
    }

    pub(crate) fn cancel_operation(&self, operation_id: &str) -> Result<bool, String> {
        validate_operation_id(operation_id)?;
        let cancellation = self
            .lock()?
            .operations
            .get(operation_id)
            .map(|operation| operation.cancellation.clone());
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(crate) fn end_operation(&self, operation_id: &str) {
        if let Ok(mut state) = self.lock() {
            state.operations.remove(operation_id);
        }
    }

    #[cfg(test)]
    fn install_watch_for_test(&self, watcher: Box<dyn WatchHandlePort>) {
        self.state.lock().unwrap().watcher = Some(watcher);
    }
}

fn create_native_watch(
    workspace_root: &Path,
    state: Weak<Mutex<RuntimeState>>,
    scope: WorkspaceIndexScope,
) -> Result<NativeWatchHandle, String> {
    let mut watcher =
        notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
            handle_native_watch_result(&state, &scope, result);
        })
        .map_err(|error| format!("Could not monitor the workspace index: {error}"))?;
    watcher
        .watch(workspace_root, RecursiveMode::Recursive)
        .map_err(|error| format!("Could not monitor the workspace index: {error}"))?;
    Ok(NativeWatchHandle {
        watcher: Some(watcher),
    })
}

fn handle_native_watch_result(
    state: &Weak<Mutex<RuntimeState>>,
    scope: &WorkspaceIndexScope,
    result: Result<notify::Event, notify::Error>,
) {
    if matches!(&result, Ok(event) if event.kind.is_access()) {
        return;
    }
    if let Ok(event) = &result {
        if event_paths_are_benign_directory_metadata(scope, event) {
            return;
        }
        let published_index = state.upgrade().and_then(|state| {
            state.lock().ok().and_then(|state| {
                state
                    .active
                    .as_ref()
                    .filter(|active| active.scope == *scope)
                    .and_then(|active| active.index.as_ref())
                    .map(|stored| Arc::clone(&stored.index))
            })
        });
        if published_index
            .as_ref()
            .is_some_and(|index| event_paths_match_published_index(scope, index, &event.paths))
        {
            return;
        }
    }
    if let Some(state) = state.upgrade() {
        invalidate_exact_binding(&state, scope, false);
    }
}

fn event_paths_are_benign_directory_metadata(
    scope: &WorkspaceIndexScope,
    event: &notify::Event,
) -> bool {
    matches!(
        event.kind,
        notify::EventKind::Modify(notify::event::ModifyKind::Metadata(_))
    ) && !event.paths.is_empty()
        && event.paths.iter().all(|path| {
            path.strip_prefix(&scope.workspace_root).is_ok()
                && std::fs::symlink_metadata(path)
                    .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        })
}

fn event_paths_match_published_index(
    scope: &WorkspaceIndexScope,
    index: &WorkspaceIndex,
    paths: &[PathBuf],
) -> bool {
    !paths.is_empty()
        && paths
            .iter()
            .all(|path| event_path_matches_published_index(scope, index, path))
}

fn event_path_matches_published_index(
    scope: &WorkspaceIndexScope,
    index: &WorkspaceIndex,
    path: &Path,
) -> bool {
    let Ok(relative) = path.strip_prefix(&scope.workspace_root) else {
        return false;
    };
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return false;
    }
    if metadata.is_dir() {
        return false;
    }
    let relative_path = relative.to_string_lossy().replace('\\', "/");
    if WorkspaceFileKind::classify(Path::new(&relative_path)) != Some(WorkspaceFileKind::Markdown) {
        return metadata.is_file();
    }
    let Some(expected) = index.content_for_relative_path(&relative_path) else {
        return false;
    };
    let Ok(file) = open_regular_file_without_following_links(path) else {
        return false;
    };
    let mut bytes = Vec::with_capacity(expected.len().min(index.max_file_bytes()));
    let mut bounded = file.take((index.max_file_bytes() as u64).saturating_add(1));
    bounded.read_to_end(&mut bytes).is_ok() && bytes == expected.as_bytes()
}

fn stop_watch(watcher: Option<Box<dyn WatchHandlePort>>) {
    if let Some(mut watcher) = watcher {
        watcher.stop();
    }
}

fn invalidate_exact_binding(
    state: &Arc<Mutex<RuntimeState>>,
    scope: &WorkspaceIndexScope,
    clear_active: bool,
) {
    let Ok(mut state) = state.lock() else {
        return;
    };
    if !state
        .active
        .as_ref()
        .is_some_and(|active| active.scope == *scope)
    {
        return;
    }
    if invalidate_locked(&mut state, scope).is_ok() && clear_active {
        state.active = None;
    }
}

fn invalidate_locked(state: &mut RuntimeState, scope: &WorkspaceIndexScope) -> Result<(), String> {
    let generation = allocate_generation(state, scope.generation)?;
    let active = state
        .active
        .as_mut()
        .ok_or_else(|| "Workspace index is not active".to_string())?;
    active.scope.generation = generation;
    active.index = None;
    cancel_operations(&mut state.operations, Some((scope, OperationKind::Rebuild)));
    cancel_operations(&mut state.operations, Some((scope, OperationKind::Query)));
    Ok(())
}

fn allocate_generation(state: &mut RuntimeState, floor: u64) -> Result<u64, String> {
    let generation = state
        .next_generation
        .max(floor)
        .checked_add(1)
        .ok_or_else(|| "Workspace index generation is exhausted".to_string())?;
    state.next_generation = generation;
    Ok(generation)
}

fn scope_matches(
    scope: &WorkspaceIndexScope,
    workspace_token: &str,
    workspace_root: &Path,
) -> bool {
    scope.workspace_token == workspace_token && scope.workspace_root == workspace_root
}

fn lease_matches(scope: &WorkspaceIndexScope, lease: &WorkspaceIndexLease) -> bool {
    scope.generation == lease.generation
        && scope.workspace_token == lease.workspace_token
        && scope.workspace_root == lease.workspace_root
}

fn cancel_operations(
    operations: &mut std::collections::HashMap<String, ActiveOperation>,
    filter: Option<(&WorkspaceIndexScope, OperationKind)>,
) {
    for operation in operations.values() {
        if filter.is_none_or(|(scope, kind)| operation.kind == kind && operation.scope == *scope) {
            operation.cancellation.cancel();
        }
    }
}

fn validate_operation_id(operation_id: &str) -> Result<(), String> {
    if operation_id.is_empty()
        || operation_id.len() > 128
        || !operation_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err("Workspace index operation ID is invalid".to_string());
    }
    Ok(())
}

fn new_deadline_bound_cancellation() -> (CancellationToken, Instant) {
    let deadline = Instant::now() + MAX_OPERATION_DURATION;
    let cancellation = CancellationToken::with_deadline(deadline);
    (cancellation, deadline)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::workspace_index::{build_index, BuildReport, IndexDocument, IndexLimits};
    use tempfile::tempdir;

    use super::*;

    fn index() -> (WorkspaceIndex, BuildReport) {
        build_index(
            vec![IndexDocument {
                relative_path: "note.md".to_string(),
                content: "needle".to_string(),
            }],
            IndexLimits::default(),
            &CancellationToken::new(),
        )
        .completed()
        .unwrap()
    }

    #[test]
    fn a_rebuild_generation_cannot_publish_after_its_scope_is_invalidated() {
        let runtime = WorkspaceIndexRuntime::default();
        let directory = tempdir().unwrap();
        let root = directory.path();
        let build = runtime
            .begin_rebuild("workspace-1", &root, "build-1")
            .unwrap();
        let (index, _) = index();

        runtime.invalidate("workspace-1", &root).unwrap();

        assert!(!runtime.publish_rebuild(&build, index).unwrap());
        assert!(!runtime
            .is_result_current("workspace-1", &root, build.generation)
            .unwrap());
    }

    #[test]
    fn a_new_rebuild_discards_the_previous_generation_and_cancels_its_work() {
        let runtime = WorkspaceIndexRuntime::default();
        let directory = tempdir().unwrap();
        let root = directory.path();
        let first = runtime
            .begin_rebuild("workspace-1", &root, "build-1")
            .unwrap();
        let (first_index, _) = index();
        assert!(runtime.publish_rebuild(&first, first_index).unwrap());

        let second = runtime
            .begin_rebuild("workspace-1", &root, "build-2")
            .unwrap();

        assert!(first.cancellation.is_cancelled());
        assert!(!runtime
            .is_result_current("workspace-1", &root, second.generation)
            .unwrap());
        assert!(second.generation > first.generation);
    }

    #[test]
    fn discard_all_does_not_reuse_a_generation_for_the_same_workspace() {
        let runtime = WorkspaceIndexRuntime::default();
        let directory = tempdir().unwrap();
        let first = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-1")
            .unwrap();

        runtime.discard_all();
        let second = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-2")
            .unwrap();

        assert!(second.generation > first.generation);
    }

    #[test]
    fn rebuild_generation_advances_past_every_invalidation_generation() {
        let runtime = WorkspaceIndexRuntime::default();
        let directory = tempdir().unwrap();
        let first = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-1")
            .unwrap();

        assert!(runtime.invalidate("workspace-1", directory.path()).unwrap());
        let invalidated_generation = runtime
            .current_generation("workspace-1", directory.path())
            .unwrap()
            .unwrap();
        let second = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-2")
            .unwrap();

        assert!(invalidated_generation > first.generation);
        assert!(second.generation > invalidated_generation);
    }

    #[test]
    fn query_leases_are_rejected_after_discard() {
        let runtime = WorkspaceIndexRuntime::default();
        let directory = tempdir().unwrap();
        let root = directory.path();
        let build = runtime
            .begin_rebuild("workspace-1", &root, "build-1")
            .unwrap();
        let (index, _) = index();
        assert!(runtime.publish_rebuild(&build, index).unwrap());
        let query = runtime
            .begin_query("workspace-1", &root, "query-1")
            .unwrap();

        assert!(runtime.discard("workspace-1", &root).unwrap());
        assert!(!runtime.is_current(&query.lease).unwrap());
        assert!(query.lease.cancellation.is_cancelled());
    }

    #[test]
    fn an_expired_lease_cannot_publish_or_remain_current() {
        let runtime = WorkspaceIndexRuntime::default();
        let directory = tempdir().unwrap();
        let root = directory.path();
        let mut build = runtime
            .begin_rebuild("workspace-1", &root, "build-1")
            .unwrap();
        build.deadline = Instant::now();
        let (index, _) = index();

        assert!(build.is_cancelled());
        assert!(!runtime.publish_rebuild(&build, index).unwrap());
        assert!(!runtime.is_current(&build).unwrap());
    }

    #[test]
    fn a_late_callback_from_a_replaced_watcher_cannot_invalidate_the_new_binding() {
        let directory = tempdir().unwrap();
        let runtime = WorkspaceIndexRuntime::default();
        let first = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-1")
            .unwrap();
        let second = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-2")
            .unwrap();
        let stale_scope = WorkspaceIndexScope {
            workspace_token: first.workspace_token,
            workspace_root: first.workspace_root,
            generation: first.generation,
        };

        invalidate_exact_binding(&runtime.state, &stale_scope, false);

        assert!(!second.cancellation.is_cancelled());
        assert_eq!(
            runtime
                .current_generation("workspace-1", directory.path())
                .unwrap(),
            Some(second.generation)
        );
    }

    #[test]
    fn watcher_access_events_preserve_the_binding_while_other_results_invalidate_it() {
        let directory = tempdir().unwrap();

        for (result, should_invalidate) in [
            (
                Ok(notify::Event::new(notify::EventKind::Access(
                    notify::event::AccessKind::Any,
                ))),
                false,
            ),
            (
                Ok(notify::Event::new(notify::EventKind::Modify(
                    notify::event::ModifyKind::Any,
                ))),
                true,
            ),
            (Ok(notify::Event::new(notify::EventKind::Any)), true),
            (
                Err(notify::Error::generic("injected watcher overflow")),
                true,
            ),
        ] {
            let runtime = WorkspaceIndexRuntime::default();
            let build = runtime
                .begin_rebuild("workspace-1", directory.path(), "build-1")
                .unwrap();
            let scope = WorkspaceIndexScope {
                workspace_token: build.workspace_token.clone(),
                workspace_root: build.workspace_root.clone(),
                generation: build.generation,
            };

            handle_native_watch_result(&Arc::downgrade(&runtime.state), &scope, result);

            assert_eq!(build.cancellation.is_cancelled(), should_invalidate);
            assert_eq!(
                runtime
                    .current_generation("workspace-1", directory.path())
                    .unwrap(),
                Some(build.generation + u64::from(should_invalidate))
            );
        }
    }

    #[test]
    fn stale_watcher_events_preserve_matching_published_content_but_real_changes_invalidate() {
        let directory = tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let document = root.join("note.md");
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(&document, "needle").unwrap();
        let runtime = WorkspaceIndexRuntime::default();
        let build = runtime
            .begin_rebuild("workspace-1", &root, "build-1")
            .unwrap();
        let watcher = runtime.state.lock().unwrap().watcher.take();
        stop_watch(watcher);
        let scope = WorkspaceIndexScope {
            workspace_token: build.workspace_token.clone(),
            workspace_root: build.workspace_root.clone(),
            generation: build.generation,
        };
        let directory_metadata_event = || {
            Ok(notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Metadata(notify::event::MetadataKind::Any),
            ))
            .add_path(root.clone())
            .add_path(nested.clone()))
        };
        handle_native_watch_result(
            &Arc::downgrade(&runtime.state),
            &scope,
            directory_metadata_event(),
        );
        assert!(!build.cancellation.is_cancelled());

        let (index, _) = index();
        assert!(runtime.publish_rebuild(&build, index).unwrap());
        let event = || {
            Ok(
                notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
                    notify::event::DataChange::Content,
                )))
                .add_path(document.clone()),
            )
        };

        handle_native_watch_result(&Arc::downgrade(&runtime.state), &scope, event());
        assert!(runtime
            .is_result_current("workspace-1", &root, build.generation)
            .unwrap());

        handle_native_watch_result(
            &Arc::downgrade(&runtime.state),
            &scope,
            directory_metadata_event(),
        );
        assert!(runtime
            .is_result_current("workspace-1", &root, build.generation)
            .unwrap());

        std::fs::write(&document, "changed").unwrap();
        handle_native_watch_result(&Arc::downgrade(&runtime.state), &scope, event());
        assert!(!runtime
            .is_result_current("workspace-1", &root, build.generation)
            .unwrap());
    }

    #[test]
    fn watcher_errors_invalidate_and_cancel_the_bound_generation() {
        let directory = tempdir().unwrap();
        let runtime = WorkspaceIndexRuntime::default();
        let build = runtime
            .begin_rebuild("workspace-1", directory.path(), "build-1")
            .unwrap();
        let scope = WorkspaceIndexScope {
            workspace_token: build.workspace_token.clone(),
            workspace_root: build.workspace_root.clone(),
            generation: build.generation,
        };

        handle_native_watch_result(
            &Arc::downgrade(&runtime.state),
            &scope,
            Err(notify::Error::generic("injected watcher overflow")),
        );

        assert!(build.cancellation.is_cancelled());
        assert_eq!(
            runtime
                .current_generation("workspace-1", directory.path())
                .unwrap(),
            Some(build.generation + 1)
        );
    }

    struct CountingWatchHandle(Arc<AtomicUsize>);

    impl WatchHandlePort for CountingWatchHandle {
        fn stop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn discard_all_stops_the_bound_workspace_watcher() {
        let runtime = WorkspaceIndexRuntime::default();
        let stopped = Arc::new(AtomicUsize::new(0));
        runtime.install_watch_for_test(Box::new(CountingWatchHandle(Arc::clone(&stopped))));

        runtime.discard_all();

        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }
}
