use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use crate::state::AppState;

#[path = "workspace_snapshot.rs"]
pub(crate) mod workspace_snapshot;

#[cfg(test)]
use std::ops::{Deref, DerefMut};

#[derive(Default)]
pub(crate) struct FileAuthorizationSession {
    inner: Mutex<AuthorizationState>,
    #[cfg(test)]
    next_save_publish_error: Mutex<Option<String>>,
    #[cfg(test)]
    next_preview_retirement_error: Mutex<Option<String>>,
    #[cfg(test)]
    next_preview_retirement_unavailable_error: Mutex<Option<String>>,
}

#[cfg(test)]
struct AuthorizationGuard<'a> {
    inner: Option<MutexGuard<'a, AuthorizationState>>,
}

#[cfg(not(test))]
type AuthorizationGuard<'a> = MutexGuard<'a, AuthorizationState>;

#[cfg(test)]
impl<'a> AuthorizationGuard<'a> {
    fn new(inner: MutexGuard<'a, AuthorizationState>) -> Self {
        lock_order_test_probe::authorization_acquired();
        Self { inner: Some(inner) }
    }
}

#[cfg(test)]
impl Deref for AuthorizationGuard<'_> {
    type Target = AuthorizationState;

    fn deref(&self) -> &Self::Target {
        self.inner
            .as_deref()
            .expect("authorization guard is active")
    }
}

#[cfg(test)]
impl DerefMut for AuthorizationGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .as_deref_mut()
            .expect("authorization guard is active")
    }
}

#[cfg(test)]
impl Drop for AuthorizationGuard<'_> {
    fn drop(&mut self) {
        self.inner.take();
        lock_order_test_probe::authorization_released();
    }
}

#[cfg(test)]
pub(crate) mod lock_order_test_probe {
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum LockEvent {
        RecentRuntimeAcquired,
        RecentRuntimeReleased,
        RecentFs2Acquired,
        RecentFs2Released,
        AuthorizationAcquired,
        AuthorizationReleased,
        HtmlSitesAcquired,
        HtmlSitesReleased,
    }

    #[derive(Default)]
    struct TraceState {
        events: Vec<LockEvent>,
        recent_runtime_depth: usize,
        recent_fs2_depth: usize,
        authorization_depth: usize,
        html_sites_depth: usize,
        violation: Option<&'static str>,
    }

    thread_local! {
        static TRACE: RefCell<Option<TraceState>> = const { RefCell::new(None) };
    }

    pub(crate) fn trace<T>(operation: impl FnOnce() -> T) -> (T, Vec<LockEvent>) {
        TRACE.with(|trace| {
            assert!(
                trace.borrow().is_none(),
                "lock-order traces cannot be nested"
            );
            *trace.borrow_mut() = Some(TraceState::default());
        });
        let result = operation();
        let state = TRACE.with(|trace| {
            trace
                .borrow_mut()
                .take()
                .expect("lock-order trace is active")
        });
        assert_eq!(state.authorization_depth, 0);
        assert_eq!(state.html_sites_depth, 0);
        assert_eq!(state.recent_runtime_depth, 0);
        assert_eq!(state.recent_fs2_depth, 0);
        assert!(
            state.violation.is_none(),
            "{}",
            state.violation.unwrap_or("")
        );
        (result, state.events)
    }

    pub(crate) fn authorization_acquired() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            if state.html_sites_depth > 0 {
                state.violation = Some("authorization acquired while HTML sites guard was held");
            }
            if state.recent_runtime_depth > 0 && state.recent_fs2_depth == 0 {
                state.violation = Some("authorization acquired before fs2 while MRU was held");
            }
            state.authorization_depth += 1;
            state.events.push(LockEvent::AuthorizationAcquired);
        });
    }

    pub(crate) fn authorization_released() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            state.authorization_depth -= 1;
            state.events.push(LockEvent::AuthorizationReleased);
        });
    }

    pub(crate) fn html_sites_acquired() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            if state.authorization_depth > 0 {
                state.violation = Some("HTML sites guard acquired while authorization was held");
            }
            if state.recent_runtime_depth > 0 {
                state.violation = Some("HTML sites guard acquired while MRU was held");
            }
            state.html_sites_depth += 1;
            state.events.push(LockEvent::HtmlSitesAcquired);
        });
    }

    pub(crate) fn html_sites_released() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            state.html_sites_depth -= 1;
            state.events.push(LockEvent::HtmlSitesReleased);
        });
    }

    pub(crate) fn recent_runtime_acquired() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            if state.authorization_depth > 0
                || state.html_sites_depth > 0
                || state.recent_fs2_depth > 0
            {
                state.violation = Some("MRU acquired while a later-order lock was held");
            }
            state.recent_runtime_depth += 1;
            state.events.push(LockEvent::RecentRuntimeAcquired);
        });
    }

    pub(crate) fn recent_runtime_released() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            if state.recent_fs2_depth > 0 || state.authorization_depth > 0 {
                state.violation = Some("MRU released before later-order locks");
            }
            state.recent_runtime_depth -= 1;
            state.events.push(LockEvent::RecentRuntimeReleased);
        });
    }

    pub(crate) fn recent_fs2_acquired() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            if state.recent_runtime_depth == 0
                || state.authorization_depth > 0
                || state.html_sites_depth > 0
            {
                state.violation = Some("fs2 acquired outside the MRU lock order");
            }
            state.recent_fs2_depth += 1;
            state.events.push(LockEvent::RecentFs2Acquired);
        });
    }

    pub(crate) fn recent_fs2_released() {
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(state) = trace.as_mut() else {
                return;
            };
            if state.authorization_depth > 0 {
                state.violation = Some("fs2 released before authorization");
            }
            state.recent_fs2_depth -= 1;
            state.events.push(LockEvent::RecentFs2Released);
        });
    }

    pub(crate) fn assert_no_locks_held() {
        TRACE.with(|trace| {
            let trace = trace.borrow();
            let state = trace.as_ref().expect("lock-order trace is active");
            assert_eq!(state.authorization_depth, 0);
            assert_eq!(state.html_sites_depth, 0);
            assert_eq!(state.recent_runtime_depth, 0);
            assert_eq!(state.recent_fs2_depth, 0);
        });
    }

    pub(crate) fn assert_authorization_held_without_html_sites() {
        TRACE.with(|trace| {
            let trace = trace.borrow();
            let state = trace.as_ref().expect("lock-order trace is active");
            assert_eq!(state.authorization_depth, 1);
            assert_eq!(state.html_sites_depth, 0);
        });
    }
}

#[derive(Default)]
struct AuthorizationState {
    workspaces: HashMap<WorkspaceToken, WorkspaceGrant>,
    grants: HashMap<GrantKey, GrantLedger>,
    next_workspace_token_id: u64,
    next_document_grant_id: u64,
    next_preview_lease_id: u64,
    next_grant_sequence: u64,
}

pub(crate) struct WorkspaceCandidate {
    root: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct WorkspaceToken(u64);

struct WorkspaceGrant {
    root: PathBuf,
}

#[derive(Clone)]
pub(crate) struct AuthorizedWorkspace {
    token: WorkspaceToken,
    root: PathBuf,
}

pub(crate) struct RenamedWorkspaceEntry {
    workspace: AuthorizedWorkspace,
    old_path: PathBuf,
    new_path: PathBuf,
    is_file: bool,
}

pub(crate) struct DeletedWorkspaceEntry {
    workspace: AuthorizedWorkspace,
    deleted_path: PathBuf,
    is_file: bool,
}

pub(crate) enum AuthorizedRenameOutcome {
    ConfirmedNotCommitted {
        message: String,
    },
    Committed(RenamedWorkspaceEntry),
    RecoveryRequired {
        renamed: RenamedWorkspaceEntry,
        recovery_message: String,
    },
    Indeterminate {
        attempted: RenamedWorkspaceEntry,
        recovery_message: String,
    },
}

pub(crate) enum RenameErrorObservation {
    ConfirmedNotCommitted,
    ConfirmedCommitted,
    Indeterminate { message: String },
}

enum RenameWorkspaceEntryAuthorizationOutcome {
    Committed {
        renamed: RenamedWorkspaceEntry,
        invalidated_preview_leases: HashSet<PreviewLeaseId>,
    },
    AwaitingObservation {
        attempted: RenamedWorkspaceEntry,
        transitioned_grants: HashSet<GrantKey>,
        operation_error: String,
    },
    ConfirmedNotCommitted {
        message: String,
    },
    Indeterminate {
        attempted: RenamedWorkspaceEntry,
        invalidated_preview_leases: HashSet<PreviewLeaseId>,
        operation_error: String,
        observation_message: String,
    },
}

pub(crate) enum AuthorizedDeleteOutcome {
    ConfirmedNotCommitted {
        message: String,
    },
    Committed(DeletedWorkspaceEntry),
    RecoveryRequired {
        deleted: DeletedWorkspaceEntry,
        recovery_message: String,
    },
    Indeterminate {
        attempted: DeletedWorkspaceEntry,
        recovery_message: String,
    },
}

pub(crate) enum DeleteFileObservation {
    Present,
    Missing,
}

enum DeleteWorkspaceEntryAuthorizationOutcome {
    ConfirmedNotCommitted {
        message: String,
    },
    Committed {
        deleted: DeletedWorkspaceEntry,
        invalidated_preview_leases: HashSet<PreviewLeaseId>,
    },
    Indeterminate {
        attempted: DeletedWorkspaceEntry,
        invalidated_preview_leases: HashSet<PreviewLeaseId>,
        operation_error: String,
    },
}

pub(crate) enum AuthorizedWriteOutcome {
    Committed(PathBuf),
    Indeterminate {
        path: PathBuf,
        recovery_message: String,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum WorkspaceSnapshotSource<'a> {
    Candidate(&'a WorkspaceCandidate),
    Authorized(&'a AuthorizedWorkspace),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DocumentGrantId(u64);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PreviewLeaseId {
    generation: u64,
    document: PathBuf,
}

pub(crate) enum PreviewRetirementError {
    AuthorizationUnavailable(String),
    #[cfg(test)]
    Recoverable(String),
}

impl PreviewRetirementError {
    pub(crate) fn into_message(self) -> String {
        match self {
            Self::AuthorizationUnavailable(message) => message,
            #[cfg(test)]
            Self::Recoverable(message) => message,
        }
    }
}

pub(crate) struct AuthorizedFile {
    path: PathBuf,
    #[cfg(test)]
    origin: GrantOrigin,
}

pub(crate) struct AuthorizedPreviewScope {
    document: PathBuf,
    root: PathBuf,
    lease: PreviewLeaseId,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum GrantOrigin {
    Workspace(WorkspaceToken),
    OpenDocument(DocumentGrantId),
    SaveAs(DocumentGrantId),
    CreatedDocument(DocumentGrantId),
    Preview(PreviewLeaseId),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum GrantKey {
    ExactReadWrite(PathBuf),
    DirectoryRead(PathBuf),
    InternalAsset(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GrantStatus {
    Active,
    Suspended,
}

struct GrantLedger {
    origins: HashMap<GrantOrigin, usize>,
    status: GrantStatus,
    first_granted_sequence: u64,
}

enum PreparedGrantMutation {
    Existing { key: GrantKey, origin: GrantOrigin },
    New { key: GrantKey, ledger: GrantLedger },
}

pub(crate) struct PreparedOpenDocumentGrant<'a> {
    state: AuthorizationGuard<'a>,
    mutations: Vec<PreparedGrantMutation>,
    next_document_grant_id: u64,
    next_grant_sequence: u64,
}

impl PreparedOpenDocumentGrant<'_> {
    pub(crate) fn apply(mut self) {
        self.state.next_document_grant_id = self.next_document_grant_id;
        self.state.next_grant_sequence = self.next_grant_sequence;
        for mutation in self.mutations {
            match mutation {
                PreparedGrantMutation::Existing { key, origin } => {
                    let ledger = self
                        .state
                        .grants
                        .get_mut(&key)
                        .expect("prepared grant retains the authorization lock");
                    ledger.origins.insert(origin, 1);
                    ledger.status = GrantStatus::Active;
                }
                PreparedGrantMutation::New { key, ledger } => {
                    let replaced = self.state.grants.insert(key, ledger);
                    debug_assert!(replaced.is_none());
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RevokeOriginMode {
    All,
}

pub(crate) fn canonicalize_existing_path(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    fs::canonicalize(path.as_ref())
}

pub(crate) fn normalize_existing_path(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    canonicalize_existing_path(path).map_err(|err| format!("Cannot access path: {err}"))
}

pub(crate) fn normalize_parent_for_new_path(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = path.as_ref();
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Parent directory traversal is not allowed".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Path has no parent directory".to_string())?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "Path has no file name".to_string())?;
    let parent = normalize_existing_path(parent)?;
    Ok(parent.join(file_name))
}

pub(crate) fn normalize_file_for_write(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = path.as_ref();
    if path.exists() {
        let canonical = normalize_existing_path(path)?;
        if !canonical.is_file() {
            return Err("Destination is not a file".into());
        }
        Ok(canonical)
    } else {
        normalize_parent_for_new_path(path)
    }
}

pub(crate) fn path_is_under(child: &Path, root: &Path) -> bool {
    child == root || child.starts_with(root)
}

impl AuthorizedWorkspace {
    fn new(token: WorkspaceToken, root: PathBuf) -> Self {
        Self { token, root }
    }

    #[cfg(test)]
    pub(crate) fn token(&self) -> &WorkspaceToken {
        &self.token
    }

    pub(crate) fn wire_token(&self) -> String {
        self.token.to_wire()
    }

    #[cfg(test)]
    fn into_root(self) -> PathBuf {
        self.root
    }
}

impl RenamedWorkspaceEntry {
    pub(crate) fn workspace(&self) -> &AuthorizedWorkspace {
        &self.workspace
    }

    pub(crate) fn old_path(&self) -> &Path {
        &self.old_path
    }

    pub(crate) fn new_path(&self) -> &Path {
        &self.new_path
    }

    pub(crate) fn is_file(&self) -> bool {
        self.is_file
    }
}

impl DeletedWorkspaceEntry {
    pub(crate) fn workspace(&self) -> &AuthorizedWorkspace {
        &self.workspace
    }

    pub(crate) fn deleted_path(&self) -> &Path {
        &self.deleted_path
    }

    pub(crate) fn is_file(&self) -> bool {
        self.is_file
    }
}

impl WorkspaceToken {
    const WIRE_PREFIX: &'static str = "workspace-";

    fn to_wire(self) -> String {
        format!("{}{id}", Self::WIRE_PREFIX, id = self.0)
    }

    fn from_wire(value: &str) -> Result<Self, String> {
        let id = value
            .strip_prefix(Self::WIRE_PREFIX)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Invalid workspace token".to_string())?
            .parse::<u64>()
            .map_err(|_| "Invalid workspace token".to_string())?;
        let token = Self(id);
        if token.to_wire() != value {
            return Err("Invalid workspace token".to_string());
        }
        Ok(token)
    }
}

impl AuthorizedFile {
    fn new(path: PathBuf, origin: GrantOrigin) -> Self {
        #[cfg(not(test))]
        let _ = &origin;
        Self {
            path,
            #[cfg(test)]
            origin,
        }
    }

    #[cfg(test)]
    fn origin(&self) -> &GrantOrigin {
        &self.origin
    }

    pub(crate) fn into_path(self) -> PathBuf {
        self.path
    }
}

impl AuthorizedPreviewScope {
    pub(crate) fn document(&self) -> &Path {
        &self.document
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn lease(&self) -> &PreviewLeaseId {
        &self.lease
    }

    pub(crate) fn into_parts(self) -> (PathBuf, PathBuf, PreviewLeaseId) {
        (self.document, self.root, self.lease)
    }
}

impl GrantKey {
    fn path(&self) -> &Path {
        match self {
            Self::ExactReadWrite(path) | Self::DirectoryRead(path) | Self::InternalAsset(path) => {
                path
            }
        }
    }

    fn relocated(self, old_prefix: &Path, new_prefix: &Path) -> Self {
        fn relocated_path(path: PathBuf, old_prefix: &Path, new_prefix: &Path) -> PathBuf {
            match path.strip_prefix(old_prefix) {
                Ok(suffix) => new_prefix.join(suffix),
                Err(_) => path,
            }
        }

        match self {
            Self::ExactReadWrite(path) => {
                Self::ExactReadWrite(relocated_path(path, old_prefix, new_prefix))
            }
            Self::DirectoryRead(path) => {
                Self::DirectoryRead(relocated_path(path, old_prefix, new_prefix))
            }
            Self::InternalAsset(path) => {
                Self::InternalAsset(relocated_path(path, old_prefix, new_prefix))
            }
        }
    }
}

impl GrantLedger {
    fn new(origin: GrantOrigin, first_granted_sequence: u64) -> Self {
        Self {
            origins: HashMap::from([(origin, 1)]),
            status: GrantStatus::Active,
            first_granted_sequence,
        }
    }

    fn try_new(origin: GrantOrigin, first_granted_sequence: u64) -> Result<Self, String> {
        let mut origins = HashMap::new();
        origins
            .try_reserve(1)
            .map_err(|_| "Cannot reserve document grant origin".to_string())?;
        origins.insert(origin, 1);
        Ok(Self {
            origins,
            status: GrantStatus::Active,
            first_granted_sequence,
        })
    }

    fn is_active(&self) -> bool {
        self.status == GrantStatus::Active && !self.origins.is_empty()
    }

    fn add_origin(&mut self, origin: GrantOrigin) {
        let count = self.origins.entry(origin).or_default();
        *count = count.saturating_add(1);
        self.status = GrantStatus::Active;
    }

    fn revoke_origin(&mut self, origin: &GrantOrigin, mode: RevokeOriginMode) {
        let remove = match (self.origins.get_mut(origin), mode) {
            (Some(_), RevokeOriginMode::All) => true,
            (None, _) => false,
        };
        if remove {
            self.origins.remove(origin);
        }
    }

    fn suspend(&mut self) {
        self.status = GrantStatus::Suspended;
    }

    fn merge(&mut self, other: Self) {
        match (self.status, other.status) {
            (GrantStatus::Suspended, GrantStatus::Active) => {
                *self = other;
                return;
            }
            (GrantStatus::Active, GrantStatus::Suspended) => return,
            _ => {}
        }
        self.first_granted_sequence = self
            .first_granted_sequence
            .min(other.first_granted_sequence);
        for (origin, count) in other.origins {
            let current = self.origins.entry(origin).or_default();
            *current = current.saturating_add(count);
        }
    }
}

impl AuthorizationState {
    fn allocate_workspace_token(&mut self) -> Result<WorkspaceToken, String> {
        let id = self.next_workspace_token_id;
        self.next_workspace_token_id = id
            .checked_add(1)
            .ok_or_else(|| "Workspace authorization identifier space is exhausted".to_string())?;
        Ok(WorkspaceToken(id))
    }

    fn allocate_document_grant_id(&mut self) -> Result<DocumentGrantId, String> {
        let id = self.next_document_grant_id;
        self.next_document_grant_id = id
            .checked_add(1)
            .ok_or_else(|| "Document authorization identifier space is exhausted".to_string())?;
        Ok(DocumentGrantId(id))
    }

    fn allocate_preview_lease(&mut self, document: PathBuf) -> Result<PreviewLeaseId, String> {
        let generation = self.next_preview_lease_id;
        self.next_preview_lease_id = generation
            .checked_add(1)
            .ok_or_else(|| "HTML preview lease identifier space is exhausted".to_string())?;
        Ok(PreviewLeaseId {
            generation,
            document,
        })
    }

    fn grant(&mut self, key: GrantKey, origin: GrantOrigin) {
        if let Some(ledger) = self.grants.get_mut(&key) {
            ledger.add_origin(origin);
            return;
        }
        let sequence = self.next_grant_sequence;
        self.next_grant_sequence = self.next_grant_sequence.saturating_add(1);
        self.grants.insert(key, GrantLedger::new(origin, sequence));
    }

    fn grant_once(&mut self, key: GrantKey, origin: GrantOrigin) {
        if let Some(ledger) = self.grants.get_mut(&key) {
            ledger.origins.entry(origin).or_insert(1);
            ledger.status = GrantStatus::Active;
            return;
        }
        let sequence = self.next_grant_sequence;
        self.next_grant_sequence = self.next_grant_sequence.saturating_add(1);
        self.grants.insert(key, GrantLedger::new(origin, sequence));
    }

    fn revoke_origin(&mut self, origin: &GrantOrigin, mode: RevokeOriginMode) {
        for ledger in self.grants.values_mut() {
            ledger.revoke_origin(origin, mode);
        }
        self.grants.retain(|_, ledger| !ledger.origins.is_empty());
        if let GrantOrigin::Workspace(token) = origin {
            self.workspaces.remove(token);
        }
    }

    fn unsupported_preview_leases(&self) -> HashSet<PreviewLeaseId> {
        let preview_leases = self
            .grants
            .values()
            .flat_map(|ledger| ledger.origins.keys())
            .filter_map(|origin| match origin {
                GrantOrigin::Preview(lease) => Some(lease.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();

        preview_leases
            .into_iter()
            .filter(|lease| {
                let document_origins = self
                    .grants
                    .iter()
                    .filter(|(_, ledger)| ledger.is_active())
                    .filter_map(|(key, ledger)| match key {
                        GrantKey::ExactReadWrite(path) if path == &lease.document => {
                            Some(&ledger.origins)
                        }
                        GrantKey::DirectoryRead(root)
                            if path_is_under(&lease.document, root) =>
                        {
                            Some(&ledger.origins)
                        }
                        _ => None,
                    })
                    .flat_map(HashMap::keys)
                    .filter(|origin| !matches!(origin, GrantOrigin::Preview(_)))
                    .cloned()
                    .collect::<HashSet<_>>();

                !self.grants.iter().any(|(key, ledger)| {
                    ledger.is_active()
                        && matches!(key, GrantKey::DirectoryRead(root) | GrantKey::InternalAsset(root)
                            if path_is_under(&lease.document, root))
                        && ledger.origins.keys().any(|origin| {
                            !matches!(origin, GrantOrigin::Preview(_))
                                && document_origins.contains(origin)
                        })
                })
            })
            .collect()
    }

    #[cfg(test)]
    fn revoke_origin_and_unsupported_previews(
        &mut self,
        origin: &GrantOrigin,
        mode: RevokeOriginMode,
    ) -> HashSet<PreviewLeaseId> {
        self.revoke_origin(origin, mode);
        let mut invalidated = self.unsupported_preview_leases();
        if let GrantOrigin::Preview(lease) = origin {
            invalidated.insert(lease.clone());
        }
        for lease in &invalidated {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        invalidated
    }

    fn relocate_path_prefix(
        &mut self,
        old_prefix: &Path,
        new_prefix: &Path,
    ) -> HashSet<PreviewLeaseId> {
        let invalidated_preview_leases = self
            .grants
            .iter()
            .flat_map(|(key, ledger)| {
                ledger
                    .origins
                    .keys()
                    .filter_map(move |origin| match origin {
                        GrantOrigin::Preview(lease)
                            if path_is_under(key.path(), old_prefix)
                                || path_is_under(key.path(), new_prefix)
                                || path_is_under(&lease.document, old_prefix)
                                || path_is_under(&lease.document, new_prefix) =>
                        {
                            Some(lease.clone())
                        }
                        _ => None,
                    })
            })
            .collect::<HashSet<_>>();
        for lease in &invalidated_preview_leases {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        for workspace in self.workspaces.values_mut() {
            if let Ok(suffix) = workspace.root.strip_prefix(old_prefix) {
                workspace.root = new_prefix.join(suffix);
            }
        }
        let grants = std::mem::take(&mut self.grants);
        for (key, ledger) in grants {
            let relocated_key = key.relocated(old_prefix, new_prefix);
            if let Some(existing) = self.grants.get_mut(&relocated_key) {
                existing.merge(ledger);
            } else {
                self.grants.insert(relocated_key, ledger);
            }
        }
        invalidated_preview_leases
    }

    fn suspend_write_path(&mut self, path: &Path) -> HashSet<PreviewLeaseId> {
        let invalidated_preview_leases = self
            .grants
            .values()
            .flat_map(|ledger| {
                ledger.origins.keys().filter_map(|origin| match origin {
                    GrantOrigin::Preview(lease) if lease.document == path => Some(lease.clone()),
                    _ => None,
                })
            })
            .collect::<HashSet<_>>();

        if let Some(ledger) = self
            .grants
            .get_mut(&GrantKey::ExactReadWrite(path.to_path_buf()))
        {
            ledger.suspend();
        }
        for lease in &invalidated_preview_leases {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        invalidated_preview_leases
    }

    fn suspend_rename_path_prefixes(
        &mut self,
        old_prefix: &Path,
        new_prefix: &Path,
    ) -> HashSet<GrantKey> {
        let is_affected =
            |path: &Path| path_is_under(path, old_prefix) || path_is_under(path, new_prefix);
        let mut transitioned_grants = HashSet::new();
        for (key, ledger) in &mut self.grants {
            if matches!(
                key,
                GrantKey::ExactReadWrite(_) | GrantKey::InternalAsset(_)
            ) && is_affected(key.path())
                && ledger.status == GrantStatus::Active
            {
                ledger.suspend();
                transitioned_grants.insert(key.clone());
            }
        }
        transitioned_grants
    }

    fn restore_rename_grants(&mut self, transitioned_grants: &HashSet<GrantKey>) {
        for key in transitioned_grants {
            if let Some(ledger) = self.grants.get_mut(key) {
                if ledger.status == GrantStatus::Suspended && !ledger.origins.is_empty() {
                    ledger.status = GrantStatus::Active;
                }
            }
        }
    }

    fn finalize_indeterminate_rename(
        &mut self,
        old_prefix: &Path,
        new_prefix: &Path,
    ) -> HashSet<PreviewLeaseId> {
        self.suspend_rename_path_prefixes(old_prefix, new_prefix);
        let is_affected =
            |path: &Path| path_is_under(path, old_prefix) || path_is_under(path, new_prefix);
        let invalidated_preview_leases = self
            .grants
            .iter()
            .flat_map(|(key, ledger)| {
                ledger.origins.keys().filter_map(|origin| match origin {
                    GrantOrigin::Preview(lease)
                        if is_affected(key.path()) || is_affected(&lease.document) =>
                    {
                        Some(lease.clone())
                    }
                    _ => None,
                })
            })
            .collect::<HashSet<_>>();
        for lease in &invalidated_preview_leases {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        invalidated_preview_leases
    }

    fn suspend_delete_path_prefix(&mut self, prefix: &Path) -> HashSet<PreviewLeaseId> {
        let invalidated_preview_leases = self
            .grants
            .iter()
            .flat_map(|(key, ledger)| {
                ledger.origins.keys().filter_map(|origin| match origin {
                    GrantOrigin::Preview(lease)
                        if path_is_under(key.path(), prefix)
                            || path_is_under(&lease.document, prefix) =>
                    {
                        Some(lease.clone())
                    }
                    _ => None,
                })
            })
            .collect::<HashSet<_>>();

        for (key, ledger) in &mut self.grants {
            if matches!(
                key,
                GrantKey::ExactReadWrite(_) | GrantKey::InternalAsset(_)
            ) && path_is_under(key.path(), prefix)
            {
                ledger.suspend();
            }
        }
        for lease in &invalidated_preview_leases {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        invalidated_preview_leases
    }

    fn revoke_path_prefix(&mut self, prefix: &Path) -> HashSet<PreviewLeaseId> {
        let mut invalidated_preview_leases = self
            .grants
            .iter()
            .flat_map(|(key, ledger)| {
                ledger
                    .origins
                    .keys()
                    .filter_map(move |origin| match origin {
                        GrantOrigin::Preview(lease)
                            if path_is_under(key.path(), prefix)
                                || path_is_under(&lease.document, prefix) =>
                        {
                            Some(lease.clone())
                        }
                        _ => None,
                    })
            })
            .collect::<HashSet<_>>();
        let origins = self
            .grants
            .iter()
            .filter(|(key, _)| path_is_under(key.path(), prefix))
            .flat_map(|(_, ledger)| ledger.origins.keys().cloned())
            .collect::<HashSet<_>>();
        for origin in origins {
            self.revoke_origin(&origin, RevokeOriginMode::All);
        }
        for lease in &invalidated_preview_leases {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        self.grants
            .retain(|key, _| !path_is_under(key.path(), prefix));
        self.workspaces
            .retain(|_, workspace| !path_is_under(&workspace.root, prefix));
        let unsupported = self.unsupported_preview_leases();
        for lease in &unsupported {
            self.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        invalidated_preview_leases.extend(unsupported);
        invalidated_preview_leases
    }
}

impl FileAuthorizationSession {
    fn lock(&self) -> Result<AuthorizationGuard<'_>, String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| "Authorization state is poisoned".to_string())?;
        #[cfg(test)]
        {
            Ok(AuthorizationGuard::new(guard))
        }
        #[cfg(not(test))]
        {
            Ok(guard)
        }
    }

    #[cfg(test)]
    pub(crate) fn state_fingerprint_for_test(&self) -> String {
        let state = match self.inner.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut workspaces = state
            .workspaces
            .iter()
            .map(|(token, workspace)| format!("{token:?}:{}", workspace.root.display()))
            .collect::<Vec<_>>();
        workspaces.sort();
        let mut grants = state
            .grants
            .iter()
            .map(|(key, ledger)| {
                let mut origins = ledger
                    .origins
                    .iter()
                    .map(|(origin, count)| format!("{origin:?}:{count}"))
                    .collect::<Vec<_>>();
                origins.sort();
                format!(
                    "{key:?}:{:?}:{}:{origins:?}",
                    ledger.status, ledger.first_granted_sequence
                )
            })
            .collect::<Vec<_>>();
        grants.sort();
        format!(
            "workspaces={workspaces:?};grants={grants:?};counters={:?}",
            (
                state.next_workspace_token_id,
                state.next_document_grant_id,
                state.next_preview_lease_id,
                state.next_grant_sequence,
            )
        )
    }

    #[cfg(test)]
    pub(crate) fn exact_write_grant_snapshot_for_test(
        &self,
        path: &Path,
    ) -> Result<Option<(GrantStatus, usize)>, String> {
        let state = self.lock()?;
        Ok(state
            .grants
            .get(&GrantKey::ExactReadWrite(path.to_path_buf()))
            .map(|ledger| (ledger.status, ledger.origins.values().sum())))
    }

    #[cfg(test)]
    pub(crate) fn internal_asset_grant_snapshot_for_test(
        &self,
        path: &Path,
    ) -> Result<Option<(GrantStatus, usize)>, String> {
        let state = self.lock()?;
        Ok(state
            .grants
            .get(&GrantKey::InternalAsset(path.to_path_buf()))
            .map(|ledger| (ledger.status, ledger.origins.values().sum())))
    }

    #[cfg(test)]
    pub(crate) fn preview_lease_snapshot(&self) -> Result<HashSet<PreviewLeaseId>, String> {
        let state = self.lock()?;
        Ok(state
            .grants
            .values()
            .flat_map(|ledger| ledger.origins.keys())
            .filter_map(|origin| match origin {
                GrantOrigin::Preview(lease) => Some(lease.clone()),
                _ => None,
            })
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_preview_retirement(
        &self,
        error: impl Into<String>,
    ) -> Result<(), String> {
        *self
            .next_preview_retirement_error
            .lock()
            .map_err(|_| "Preview retirement test seam is poisoned".to_string())? =
            Some(error.into());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_preview_retirement_as_unavailable(
        &self,
        error: impl Into<String>,
    ) -> Result<(), String> {
        *self
            .next_preview_retirement_unavailable_error
            .lock()
            .map_err(|_| "Preview retirement unavailable test seam is poisoned".to_string())? =
            Some(error.into());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_save_publish(&self, error: impl Into<String>) -> Result<(), String> {
        *self
            .next_save_publish_error
            .lock()
            .map_err(|_| "Save publication test seam is poisoned".to_string())? =
            Some(error.into());
        Ok(())
    }

    fn workspace_candidate(&self, root: impl AsRef<Path>) -> Result<WorkspaceCandidate, String> {
        let root = normalize_existing_path(root)?;
        if !root.is_dir() {
            return Err("Authorized root must be a directory".into());
        }
        Ok(WorkspaceCandidate { root })
    }

    #[cfg(test)]
    pub(crate) fn workspace_candidate_for_test(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<WorkspaceCandidate, String> {
        self.workspace_candidate(root)
    }

    pub(crate) fn open_workspace<S>(
        &self,
        root: impl AsRef<Path>,
        snapshot: impl for<'a> FnOnce(WorkspaceSnapshotSource<'a>) -> Result<S, String>,
        transport: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(AuthorizedWorkspace, S), String> {
        let candidate = self.workspace_candidate(root)?;
        let snapshot = snapshot(WorkspaceSnapshotSource::Candidate(&candidate))?;
        let mut state = self.lock()?;
        transport(&candidate.root)?;
        let workspace = Self::publish_workspace(&mut state, candidate)?;
        Ok((workspace, snapshot))
    }

    pub(crate) fn open_workspace_at_canonical_root<S>(
        &self,
        root: impl AsRef<Path>,
        expected_root: &Path,
        snapshot: impl for<'a> FnOnce(WorkspaceSnapshotSource<'a>) -> Result<S, String>,
        transport: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(AuthorizedWorkspace, S), String> {
        let candidate = self.workspace_candidate(root)?;
        if candidate.root != expected_root {
            return Err("Saved workspace root changed while being restored".to_string());
        }
        let snapshot = snapshot(WorkspaceSnapshotSource::Candidate(&candidate))?;
        let mut state = self.lock()?;
        transport(&candidate.root)?;
        let workspace = Self::publish_workspace(&mut state, candidate)?;
        Ok((workspace, snapshot))
    }

    fn publish_workspace(
        state: &mut AuthorizationState,
        candidate: WorkspaceCandidate,
    ) -> Result<AuthorizedWorkspace, String> {
        let token = state.allocate_workspace_token()?;
        let origin = GrantOrigin::Workspace(token);
        state.grant(
            GrantKey::DirectoryRead(candidate.root.clone()),
            origin.clone(),
        );
        state.grant(GrantKey::InternalAsset(candidate.root.clone()), origin);
        state.workspaces.insert(
            token,
            WorkspaceGrant {
                root: candidate.root.clone(),
            },
        );
        Ok(AuthorizedWorkspace::new(token, candidate.root))
    }

    #[cfg(test)]
    fn authorize_directory_root_with<F>(
        &self,
        root: impl AsRef<Path>,
        before_commit: F,
    ) -> Result<AuthorizedWorkspace, String>
    where
        F: FnOnce(&WorkspaceCandidate) -> Result<(), String>,
    {
        let candidate = self.workspace_candidate(root)?;
        let mut state = self.lock()?;
        before_commit(&candidate)?;
        Self::publish_workspace(&mut state, candidate)
    }

    #[cfg(test)]
    fn authorize_directory_root(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<AuthorizedWorkspace, String> {
        self.authorize_directory_root_with(root, |_| Ok(()))
    }

    #[cfg(test)]
    fn authorize_file(&self, file: impl AsRef<Path>) -> Result<AuthorizedFile, String> {
        let file = normalize_existing_path(file)?;
        if !file.is_file() {
            return Err("Authorized file must be a file".into());
        }
        let mut state = self.lock()?;
        Self::publish_open_document(&mut state, file, true)
    }

    #[cfg(test)]
    pub(crate) fn open_standalone_file<S>(
        &self,
        file: impl AsRef<Path>,
        response: impl FnOnce(&Path) -> Result<S, String>,
        transport: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(AuthorizedFile, S), String> {
        let file = normalize_existing_path(file)?;
        if !file.is_file() {
            return Err("Authorized file must be a file".into());
        }
        let response = response(&file)?;
        let parent = file
            .parent()
            .ok_or_else(|| "Selected file has no parent directory".to_string())?
            .to_path_buf();
        let mut state = self.lock()?;
        transport(&parent)?;
        let authorized = Self::publish_open_document(&mut state, file, true)?;
        Ok((authorized, response))
    }

    pub(crate) fn with_prepared_open_document_grant<T>(
        &self,
        file: impl AsRef<Path>,
        operation: impl FnOnce(PreparedOpenDocumentGrant<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        let file = normalize_existing_path(file)?;
        if !file.is_file() {
            return Err("Prepared document grant target must be a file".to_string());
        }
        let parent = file
            .parent()
            .ok_or_else(|| "Prepared document grant target has no parent".to_string())?
            .to_path_buf();
        let mut state = self.lock()?;
        let document_grant_id = state.next_document_grant_id;
        let next_document_grant_id = document_grant_id
            .checked_add(1)
            .ok_or_else(|| "Document authorization identifier space is exhausted".to_string())?;
        let origin = GrantOrigin::OpenDocument(DocumentGrantId(document_grant_id));
        let keys = [
            GrantKey::ExactReadWrite(file),
            GrantKey::InternalAsset(parent),
        ];
        let new_grant_count = keys
            .iter()
            .filter(|key| !state.grants.contains_key(*key))
            .count();
        state
            .grants
            .try_reserve(new_grant_count)
            .map_err(|_| "Cannot reserve document grants".to_string())?;

        let mut mutations = Vec::new();
        mutations
            .try_reserve_exact(keys.len())
            .map_err(|_| "Cannot reserve prepared document grants".to_string())?;
        let mut next_grant_sequence = state.next_grant_sequence;
        for key in keys {
            if let Some(ledger) = state.grants.get_mut(&key) {
                ledger
                    .origins
                    .try_reserve(1)
                    .map_err(|_| "Cannot reserve document grant origin".to_string())?;
                mutations.push(PreparedGrantMutation::Existing {
                    key,
                    origin: origin.clone(),
                });
            } else {
                let ledger = GrantLedger::try_new(origin.clone(), next_grant_sequence)?;
                next_grant_sequence = next_grant_sequence
                    .checked_add(1)
                    .ok_or_else(|| "Document grant sequence is exhausted".to_string())?;
                mutations.push(PreparedGrantMutation::New { key, ledger });
            }
        }

        operation(PreparedOpenDocumentGrant {
            state,
            mutations,
            next_document_grant_id,
            next_grant_sequence,
        })
    }

    fn prepare_save_destination(
        &self,
        path: impl AsRef<Path>,
        preflight: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        let normalized = normalize_file_for_write(path)?;
        let _state = self.lock()?;
        preflight(&normalized)?;
        Ok(normalized)
    }

    fn publish_save_destination(&self, normalized: PathBuf) -> Result<AuthorizedFile, String> {
        #[cfg(test)]
        if let Some(error) = self
            .next_save_publish_error
            .lock()
            .map_err(|_| "Save publication test seam is poisoned".to_string())?
            .take()
        {
            return Err(error);
        }
        let mut state = self.lock()?;
        let origin = GrantOrigin::SaveAs(state.allocate_document_grant_id()?);
        Self::grant_exact_file(&mut state, &normalized, origin.clone(), true);
        Ok(AuthorizedFile::new(normalized, origin))
    }

    #[cfg(test)]
    fn authorize_save_destination(&self, path: impl AsRef<Path>) -> Result<AuthorizedFile, String> {
        let normalized = normalize_file_for_write(path)?;
        self.publish_save_destination(normalized)
    }

    fn grant_exact_file(
        state: &mut AuthorizationState,
        file: &Path,
        origin: GrantOrigin,
        include_internal_assets: bool,
    ) {
        state.grant(GrantKey::ExactReadWrite(file.to_path_buf()), origin.clone());
        if include_internal_assets {
            if let Some(parent) = file.parent() {
                state.grant(GrantKey::InternalAsset(parent.to_path_buf()), origin);
            }
        }
    }

    #[cfg(test)]
    fn publish_open_document(
        state: &mut AuthorizationState,
        file: PathBuf,
        include_internal_assets: bool,
    ) -> Result<AuthorizedFile, String> {
        let origin = GrantOrigin::OpenDocument(state.allocate_document_grant_id()?);
        Self::grant_exact_file(state, &file, origin.clone(), include_internal_assets);
        Ok(AuthorizedFile::new(file, origin))
    }

    #[cfg(test)]
    fn open_workspace_file(&self, path: impl AsRef<Path>) -> Result<AuthorizedFile, String> {
        let canonical = normalize_existing_path(path)?;
        if !canonical.is_file() {
            return Err("Path is not a file".into());
        }
        let mut state = self.lock()?;
        if !Self::is_existing_file_authorized(&state, &canonical) {
            return Err("File is outside the user-authorized session files and directories".into());
        }
        Self::publish_open_document(&mut state, canonical, false)
    }

    fn is_existing_file_authorized(state: &AuthorizationState, canonical: &Path) -> bool {
        state.grants.iter().any(|(key, ledger)| {
            ledger.is_active()
                && match key {
                    GrantKey::ExactReadWrite(file) => file == canonical,
                    GrantKey::DirectoryRead(root) => path_is_under(canonical, root),
                    GrantKey::InternalAsset(_) => false,
                }
        })
    }

    fn file_for_read(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let canonical = normalize_existing_path(path)?;
        if !canonical.is_file() {
            return Err("Path is not a file".into());
        }
        let state = self.lock()?;
        if Self::is_existing_file_authorized(&state, &canonical) {
            Ok(canonical)
        } else {
            Err("File is outside the user-authorized session files and directories".into())
        }
    }

    fn file_for_watch(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let normalized = normalize_file_for_write(path)?;
        let state = self.lock()?;
        if state.grants.iter().any(|(key, ledger)| {
            ledger.is_active()
                && match key {
                    GrantKey::ExactReadWrite(file) => file == &normalized,
                    GrantKey::DirectoryRead(root) => path_is_under(&normalized, root),
                    GrantKey::InternalAsset(_) => false,
                }
        }) {
            Ok(normalized)
        } else {
            Err("File is outside the user-authorized session files and directories".into())
        }
    }

    #[cfg(test)]
    fn file_for_write(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let normalized = normalize_file_for_write(path)?;
        let state = self.lock()?;
        if state.grants.iter().any(|(key, ledger)| {
            ledger.is_active()
                && matches!(key, GrantKey::ExactReadWrite(file) if file == &normalized)
        }) {
            Ok(normalized)
        } else {
            Err("Destination file has not been explicitly authorized by open, workspace selection, or save-as".into())
        }
    }

    fn write_document(
        &self,
        path: impl AsRef<Path>,
        preflight: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        let path = normalize_file_for_write(path)?;
        let state = self.lock()?;
        if !state.grants.iter().any(|(key, ledger)| {
            ledger.is_active() && matches!(key, GrantKey::ExactReadWrite(file) if file == &path)
        }) {
            return Err("Destination file has not been explicitly authorized by open, workspace selection, or save-as".into());
        }
        preflight(&path)?;
        Ok(path)
    }

    fn suspend_write_file(&self, path: &Path) -> Result<HashSet<PreviewLeaseId>, String> {
        let mut state = self.lock()?;
        Ok(state.suspend_write_path(path))
    }

    fn directory_for_read(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let canonical = normalize_existing_path(path)?;
        if !canonical.is_dir() {
            return Err("Path is not a directory".into());
        }
        let state = self.lock()?;
        if state.grants.iter().any(|(key, ledger)| {
            ledger.is_active()
                && matches!(key, GrantKey::DirectoryRead(root) if path_is_under(&canonical, root))
        }) {
            Ok(canonical)
        } else {
            Err("Directory is outside the user-authorized session roots".into())
        }
    }

    fn workspace_for_token(
        state: &AuthorizationState,
        token: &WorkspaceToken,
    ) -> Option<AuthorizedWorkspace> {
        let workspace = state.workspaces.get(token)?;
        state
            .grants
            .get(&GrantKey::DirectoryRead(workspace.root.clone()))
            .filter(|ledger| {
                ledger.is_active() && ledger.origins.contains_key(&GrantOrigin::Workspace(*token))
            })?;
        Some(AuthorizedWorkspace::new(*token, workspace.root.clone()))
    }

    fn authorized_workspace_root_for_token(
        &self,
        workspace_token: &str,
        root: impl AsRef<Path>,
    ) -> Result<AuthorizedWorkspace, String> {
        let token = WorkspaceToken::from_wire(workspace_token)?;
        let canonical = normalize_existing_path(root)?;
        if !canonical.is_dir() {
            return Err("Path is not a directory".into());
        }
        let state = self.lock()?;
        let workspace = Self::workspace_for_token(&state, &token)
            .ok_or_else(|| "Workspace authorization is no longer active".to_string())?;
        if workspace.root != canonical {
            return Err("Directory does not match the selected workspace".into());
        }
        Ok(workspace)
    }

    pub(crate) fn ensure_workspace_is_current(
        &self,
        workspace: &AuthorizedWorkspace,
    ) -> Result<(), String> {
        let state = self.lock()?;
        let active_workspace = Self::workspace_for_token(&state, &workspace.token)
            .ok_or_else(|| "Workspace authorization is no longer active".to_string())?;
        if active_workspace.root != workspace.root {
            return Err("Workspace authorization does not match the selected root".into());
        }
        Ok(())
    }

    fn authorized_workspace_directory_for_token(
        &self,
        workspace_token: &str,
        path: impl AsRef<Path>,
    ) -> Result<(PathBuf, AuthorizedWorkspace), String> {
        let token = WorkspaceToken::from_wire(workspace_token)?;
        let canonical = normalize_existing_path(path)?;
        if !canonical.is_dir() {
            return Err("Path is not a directory".into());
        }
        let state = self.lock()?;
        let workspace = Self::workspace_for_token(&state, &token)
            .ok_or_else(|| "Workspace authorization is no longer active".to_string())?;
        if !path_is_under(&canonical, &workspace.root) {
            return Err("Directory is outside the selected workspace".into());
        }
        Ok((canonical, workspace))
    }

    pub(crate) fn create_workspace_file(
        &self,
        workspace: &AuthorizedWorkspace,
        parent_path: impl AsRef<Path>,
        file_name: &str,
        create: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(AuthorizedWorkspace, AuthorizedFile), String> {
        let mut components = Path::new(file_name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err("Workspace entry name is invalid".into());
        }
        let parent = normalize_existing_path(parent_path)?;
        if !parent.is_dir() {
            return Err("Path is not a directory".into());
        }
        let target = parent.join(file_name);

        let mut state = self.lock()?;
        let active_workspace = Self::workspace_for_token(&state, &workspace.token)
            .ok_or_else(|| "Workspace authorization is no longer active".to_string())?;
        if active_workspace.root != workspace.root {
            return Err("Workspace authorization does not match the selected root".into());
        }
        if !path_is_under(&parent, &active_workspace.root) {
            return Err("Directory is outside the selected workspace".into());
        }
        if target.exists() {
            return Err("Workspace entry already exists".into());
        }

        let origin = GrantOrigin::CreatedDocument(state.allocate_document_grant_id()?);
        create(&target)?;
        Self::grant_exact_file(&mut state, &target, origin.clone(), true);

        Ok((active_workspace, AuthorizedFile::new(target, origin)))
    }

    pub(crate) fn create_workspace_directory(
        &self,
        workspace: &AuthorizedWorkspace,
        parent_path: impl AsRef<Path>,
        directory_name: &str,
        create: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(AuthorizedWorkspace, PathBuf), String> {
        let mut components = Path::new(directory_name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err("Workspace entry name is invalid".into());
        }
        let parent = normalize_existing_path(parent_path)?;
        if !parent.is_dir() {
            return Err("Path is not a directory".into());
        }
        let target = parent.join(directory_name);

        let state = self.lock()?;
        let active_workspace = Self::workspace_for_token(&state, &workspace.token)
            .ok_or_else(|| "Workspace authorization is no longer active".to_string())?;
        if active_workspace.root != workspace.root {
            return Err("Workspace authorization does not match the selected root".into());
        }
        if !path_is_under(&parent, &active_workspace.root) {
            return Err("Directory is outside the selected workspace".into());
        }
        if target.exists() {
            return Err("Workspace entry already exists".into());
        }

        create(&target)?;
        Ok((active_workspace, target))
    }

    fn workspace_entry_for_mutation_locked(
        state: &AuthorizationState,
        token: &WorkspaceToken,
        path: impl AsRef<Path>,
    ) -> Result<(PathBuf, AuthorizedWorkspace), String> {
        let canonical = normalize_existing_path(path)?;
        if !canonical.is_file() && !canonical.is_dir() {
            return Err("Workspace entry is not a file or directory".into());
        }
        let workspace = Self::workspace_for_token(state, token)
            .ok_or_else(|| "Workspace authorization is no longer active".to_string())?;
        if !path_is_under(&canonical, &workspace.root) {
            return Err("Workspace entry is outside the selected workspace".into());
        }
        if state
            .workspaces
            .keys()
            .filter_map(|active_token| Self::workspace_for_token(state, active_token))
            .any(|active_workspace| canonical == active_workspace.root)
        {
            return Err("Cannot modify workspace root".into());
        }
        Ok((canonical, workspace))
    }

    #[cfg(test)]
    fn workspace_entry_for_mutation(
        &self,
        token: &WorkspaceToken,
        path: impl AsRef<Path>,
    ) -> Result<(PathBuf, PathBuf), String> {
        let state = self.lock()?;
        let (entry, workspace) = Self::workspace_entry_for_mutation_locked(&state, token, path)?;
        Ok((entry, workspace.into_root()))
    }

    fn relocate_workspace_entry(
        &self,
        workspace_token: &str,
        source_path: impl AsRef<Path>,
        target_path: impl FnOnce(&Path, bool, &AuthorizedWorkspace) -> Result<PathBuf, String>,
        rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<RenameWorkspaceEntryAuthorizationOutcome, String> {
        let token = WorkspaceToken::from_wire(workspace_token)?;
        let mut state = self.lock()?;
        let (source, workspace) =
            Self::workspace_entry_for_mutation_locked(&state, &token, source_path)?;
        let is_file = source.is_file();
        let requested_target = target_path(&source, is_file, &workspace)?;
        let target_name = requested_target
            .file_name()
            .ok_or_else(|| "Workspace entry destination is invalid".to_string())?;
        let mut components = Path::new(target_name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err("Workspace entry name is invalid".into());
        }
        let requested_parent = requested_target
            .parent()
            .ok_or_else(|| "Workspace entry destination has no parent".to_string())?;
        let parent = normalize_existing_path(requested_parent)?;
        if !parent.is_dir() {
            return Err("Move destination is not a directory".into());
        }
        if !path_is_under(&parent, &workspace.root) {
            return Err("Move destination is outside the selected workspace".into());
        }
        if !is_file && path_is_under(&parent, &source) {
            return Err("Cannot move a folder into itself or one of its descendants".into());
        }
        let target = parent.join(target_name);
        if target == source {
            return Err("Workspace entry is already in that folder".into());
        }
        match fs::symlink_metadata(&target) {
            Ok(_) => return Err("Workspace entry already exists".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot access rename destination: {error}")),
        }

        let entry = RenamedWorkspaceEntry {
            workspace,
            old_path: source,
            new_path: target,
            is_file,
        };
        match rename(&entry.old_path, &entry.new_path) {
            Ok(()) => {
                let invalidated_preview_leases =
                    state.relocate_path_prefix(&entry.old_path, &entry.new_path);
                Ok(RenameWorkspaceEntryAuthorizationOutcome::Committed {
                    renamed: entry,
                    invalidated_preview_leases,
                })
            }
            Err(operation_error) => {
                let transitioned_grants =
                    state.suspend_rename_path_prefixes(&entry.old_path, &entry.new_path);
                Ok(
                    RenameWorkspaceEntryAuthorizationOutcome::AwaitingObservation {
                        attempted: entry,
                        transitioned_grants,
                        operation_error,
                    },
                )
            }
        }
    }

    fn rename_workspace_entry(
        &self,
        workspace_token: &str,
        source_path: impl AsRef<Path>,
        new_name: impl FnOnce(&Path, bool) -> Result<String, String>,
        rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<RenameWorkspaceEntryAuthorizationOutcome, String> {
        self.relocate_workspace_entry(
            workspace_token,
            source_path,
            |source, is_file, _workspace| {
                let target_name = new_name(source, is_file)?;
                let mut components = Path::new(&target_name).components();
                if target_name.contains('/')
                    || target_name.contains('\\')
                    || !matches!(components.next(), Some(Component::Normal(_)))
                    || components.next().is_some()
                {
                    return Err("Workspace entry name is invalid".into());
                }
                let parent = source
                    .parent()
                    .ok_or_else(|| "Workspace entry has no parent".to_string())?;
                Ok(parent.join(target_name))
            },
            rename,
        )
    }

    fn move_workspace_entry(
        &self,
        workspace_token: &str,
        source_path: impl AsRef<Path>,
        destination_parent_path: impl AsRef<Path>,
        rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<RenameWorkspaceEntryAuthorizationOutcome, String> {
        let destination_parent = normalize_existing_path(destination_parent_path)?;
        if !destination_parent.is_dir() {
            return Err("Move destination is not a directory".into());
        }
        self.relocate_workspace_entry(
            workspace_token,
            source_path,
            move |source, _is_file, _workspace| {
                let name = source
                    .file_name()
                    .ok_or_else(|| "Workspace entry name is invalid".to_string())?;
                Ok(destination_parent.join(name))
            },
            rename,
        )
    }

    fn reconcile_rename_after_error(
        &self,
        attempted: RenamedWorkspaceEntry,
        transitioned_grants: HashSet<GrantKey>,
        operation_error: String,
        observation: RenameErrorObservation,
    ) -> Result<RenameWorkspaceEntryAuthorizationOutcome, String> {
        let mut state = self.lock()?;
        match observation {
            RenameErrorObservation::ConfirmedNotCommitted => {
                state.restore_rename_grants(&transitioned_grants);
                Ok(
                    RenameWorkspaceEntryAuthorizationOutcome::ConfirmedNotCommitted {
                        message: operation_error,
                    },
                )
            }
            RenameErrorObservation::ConfirmedCommitted => {
                state.restore_rename_grants(&transitioned_grants);
                let invalidated_preview_leases =
                    state.relocate_path_prefix(&attempted.old_path, &attempted.new_path);
                Ok(RenameWorkspaceEntryAuthorizationOutcome::Committed {
                    renamed: attempted,
                    invalidated_preview_leases,
                })
            }
            RenameErrorObservation::Indeterminate { message } => {
                let invalidated_preview_leases =
                    state.finalize_indeterminate_rename(&attempted.old_path, &attempted.new_path);
                Ok(RenameWorkspaceEntryAuthorizationOutcome::Indeterminate {
                    attempted,
                    invalidated_preview_leases,
                    operation_error,
                    observation_message: message,
                })
            }
        }
    }

    fn delete_workspace_entry(
        &self,
        workspace_token: &str,
        source_path: impl AsRef<Path>,
        delete: impl FnOnce(&Path, bool) -> Result<(), String>,
        observe_file_after_error: impl FnOnce(&Path) -> Result<DeleteFileObservation, String>,
    ) -> Result<DeleteWorkspaceEntryAuthorizationOutcome, String> {
        let token = WorkspaceToken::from_wire(workspace_token)?;
        let mut state = self.lock()?;
        let (source, workspace) =
            Self::workspace_entry_for_mutation_locked(&state, &token, source_path)?;
        let is_file = source.is_file();
        let deleted = DeletedWorkspaceEntry {
            workspace,
            deleted_path: source,
            is_file,
        };

        if let Err(message) = delete(&deleted.deleted_path, is_file) {
            if !is_file {
                let invalidated_preview_leases =
                    state.suspend_delete_path_prefix(&deleted.deleted_path);
                return Ok(DeleteWorkspaceEntryAuthorizationOutcome::Indeterminate {
                    attempted: deleted,
                    invalidated_preview_leases,
                    operation_error: message,
                });
            }
            match observe_file_after_error(&deleted.deleted_path) {
                Ok(DeleteFileObservation::Present) => {
                    return Ok(
                        DeleteWorkspaceEntryAuthorizationOutcome::ConfirmedNotCommitted { message },
                    );
                }
                Ok(DeleteFileObservation::Missing) => {}
                Err(observation_error) => {
                    let invalidated_preview_leases =
                        state.suspend_delete_path_prefix(&deleted.deleted_path);
                    return Ok(DeleteWorkspaceEntryAuthorizationOutcome::Indeterminate {
                        attempted: deleted,
                        invalidated_preview_leases,
                        operation_error: format!(
                            "{message}; delete outcome observation failed: {observation_error}"
                        ),
                    });
                }
            }
        }
        let invalidated_preview_leases = state.revoke_path_prefix(&deleted.deleted_path);
        Ok(DeleteWorkspaceEntryAuthorizationOutcome::Committed {
            deleted,
            invalidated_preview_leases,
        })
    }

    fn relocate_path_prefix(
        &self,
        old_prefix: &Path,
        new_prefix: &Path,
    ) -> Result<HashSet<PreviewLeaseId>, String> {
        let mut state = self.lock()?;
        Ok(state.relocate_path_prefix(old_prefix, new_prefix))
    }

    fn revoke_path_prefix(&self, prefix: &Path) -> Result<HashSet<PreviewLeaseId>, String> {
        let mut state = self.lock()?;
        Ok(state.revoke_path_prefix(prefix))
    }

    fn is_authorized_image(&self, canonical: &Path) -> Result<bool, String> {
        let state = self.lock()?;
        Ok(state.grants.iter().any(|(key, ledger)| {
            ledger.is_active()
                && match key {
                    GrantKey::DirectoryRead(root) | GrantKey::InternalAsset(root) => {
                        path_is_under(canonical, root)
                    }
                    GrantKey::ExactReadWrite(_) => false,
                }
        }))
    }

    fn preview_scope_for(&self, file: impl AsRef<Path>) -> Result<AuthorizedPreviewScope, String> {
        let document = normalize_existing_path(file)?;
        if !document.is_file() {
            return Err("Path is not a file".into());
        }
        let mut state = self.lock()?;
        let document_origins = state
            .grants
            .iter()
            .filter(|(_, ledger)| ledger.is_active())
            .filter_map(|(key, ledger)| match key {
                GrantKey::ExactReadWrite(path) if path == &document => Some(&ledger.origins),
                GrantKey::DirectoryRead(root) if path_is_under(&document, root) => {
                    Some(&ledger.origins)
                }
                _ => None,
            })
            .flat_map(HashMap::keys)
            .cloned()
            .collect::<HashSet<_>>();
        let root = state
            .grants
            .iter()
            .filter(|(_, ledger)| ledger.is_active())
            .filter_map(|(key, ledger)| match key {
                GrantKey::DirectoryRead(root) | GrantKey::InternalAsset(root)
                    if path_is_under(&document, root)
                        && ledger
                            .origins
                            .keys()
                            .any(|origin| document_origins.contains(origin)) =>
                {
                    Some(root)
                }
                _ => None,
            })
            .max_by_key(|root| root.components().count())
            .cloned()
            .ok_or_else(|| {
                "File is outside the user-authorized session files and directories".to_string()
            })?;
        let lease = state.allocate_preview_lease(document.clone())?;
        state.grant_once(
            GrantKey::InternalAsset(root.clone()),
            GrantOrigin::Preview(lease.clone()),
        );
        Ok(AuthorizedPreviewScope {
            document,
            root,
            lease,
        })
    }

    #[cfg(test)]
    fn revoke_origin(&self, origin: &GrantOrigin, mode: RevokeOriginMode) -> Result<(), String> {
        let mut state = self.lock()?;
        state.revoke_origin(origin, mode);
        Ok(())
    }

    fn retire_preview_leases(
        &self,
        leases: &HashSet<PreviewLeaseId>,
    ) -> Result<(), PreviewRetirementError> {
        if leases.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        if let Some(error) = self
            .next_preview_retirement_unavailable_error
            .lock()
            .map_err(|_| {
                PreviewRetirementError::AuthorizationUnavailable(
                    "Preview retirement unavailable test seam is poisoned".to_string(),
                )
            })?
            .take()
        {
            return Err(PreviewRetirementError::AuthorizationUnavailable(error));
        }
        let mut state = self
            .lock()
            .map_err(PreviewRetirementError::AuthorizationUnavailable)?;
        #[cfg(test)]
        if let Some(error) = self
            .next_preview_retirement_error
            .lock()
            .map_err(|_| {
                PreviewRetirementError::Recoverable(
                    "Preview retirement test seam is poisoned".to_string(),
                )
            })?
            .take()
        {
            return Err(PreviewRetirementError::Recoverable(error));
        }
        for lease in leases {
            state.revoke_origin(&GrantOrigin::Preview(lease.clone()), RevokeOriginMode::All);
        }
        Ok(())
    }

    #[cfg(test)]
    fn revoke_authorized_file(
        &self,
        file: &AuthorizedFile,
    ) -> Result<HashSet<PreviewLeaseId>, String> {
        let mut state = self.lock()?;
        Ok(state.revoke_origin_and_unsupported_previews(&file.origin, RevokeOriginMode::All))
    }
}

fn invalidate_preview_leases_after_authorization(
    state: &AppState,
    invalidated_preview_leases: &HashSet<PreviewLeaseId>,
) -> Result<(), String> {
    match state
        .html_preview_server
        .invalidate_preview_leases(invalidated_preview_leases)
    {
        Ok(()) => Ok(()),
        Err(recovery) => {
            let (error, drained_leases) = recovery.into_parts();
            retire_preview_leases_inner(state, &drained_leases)
                .map_err(PreviewRetirementError::into_message)?;
            Err(error)
        }
    }
}

fn reconcile_indeterminate_write_with_preview_inner(
    state: &AppState,
    path: PathBuf,
    mut recovery_message: String,
    invalidate_preview: impl FnOnce(&HashSet<PreviewLeaseId>) -> Result<(), String>,
) -> AuthorizedWriteOutcome {
    match state.file_authorization().suspend_write_file(&path) {
        Ok(invalidated_preview_leases) => {
            if let Err(cleanup_error) = invalidate_preview(&invalidated_preview_leases) {
                recovery_message.push_str(" Preview invalidation also failed: ");
                recovery_message.push_str(&cleanup_error);
            }
        }
        Err(authorization_error) => {
            let preview_shutdown = state.html_preview_server.stop_all_sites();
            recovery_message.push_str(" Authorization suspension also failed: ");
            recovery_message.push_str(&authorization_error);
            recovery_message.push_str(". All HTML preview sites were stopped.");
            if let Err(shutdown_error) = preview_shutdown {
                recovery_message.push_str(" Preview shutdown reported: ");
                recovery_message.push_str(&shutdown_error);
            }
        }
    }
    AuthorizedWriteOutcome::Indeterminate {
        path,
        recovery_message,
    }
}

fn write_authorized_document_with_preview_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    preflight: impl FnOnce(&Path) -> Result<(), String>,
    write: impl FnOnce(&Path) -> Result<(), String>,
    invalidate_preview: impl FnOnce(&HashSet<PreviewLeaseId>) -> Result<(), String>,
) -> Result<AuthorizedWriteOutcome, String> {
    let path = state.file_authorization().write_document(path, preflight)?;
    match write(&path) {
        Ok(()) => Ok(AuthorizedWriteOutcome::Committed(path)),
        Err(recovery_message) => Ok(reconcile_indeterminate_write_with_preview_inner(
            state,
            path,
            recovery_message,
            invalidate_preview,
        )),
    }
}

pub(crate) fn write_authorized_document_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    preflight: impl FnOnce(&Path) -> Result<(), String>,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<AuthorizedWriteOutcome, String> {
    write_authorized_document_with_preview_inner(
        state,
        path,
        preflight,
        write,
        |invalidated_preview_leases| {
            invalidate_preview_leases_after_authorization(state, invalidated_preview_leases)
        },
    )
}

pub(crate) fn save_document_as_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    preflight: impl FnOnce(&Path) -> Result<(), String>,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<AuthorizedWriteOutcome, String> {
    let path = state
        .file_authorization()
        .prepare_save_destination(path, preflight)?;
    if let Err(recovery_message) = write(&path) {
        return Ok(reconcile_indeterminate_write_with_preview_inner(
            state,
            path,
            recovery_message,
            |invalidated_preview_leases| {
                invalidate_preview_leases_after_authorization(state, invalidated_preview_leases)
            },
        ));
    }

    match state
        .file_authorization()
        .publish_save_destination(path.clone())
    {
        Ok(authorized) => Ok(AuthorizedWriteOutcome::Committed(authorized.into_path())),
        Err(error) => Ok(reconcile_indeterminate_write_with_preview_inner(
            state,
            path,
            format!(
                "File contents were written, but save-as authorization could not be committed: {error}. Reopen and inspect the file before retrying."
            ),
            |invalidated_preview_leases| {
                invalidate_preview_leases_after_authorization(state, invalidated_preview_leases)
            },
        )),
    }
}

fn finish_authorized_workspace_entry_relocation(
    state: &AppState,
    outcome: RenameWorkspaceEntryAuthorizationOutcome,
    observe_after_error: impl FnOnce(&RenamedWorkspaceEntry) -> RenameErrorObservation,
    invalidate_preview: impl FnOnce(&HashSet<PreviewLeaseId>) -> Result<(), String>,
) -> Result<AuthorizedRenameOutcome, String> {
    let outcome = match outcome {
        RenameWorkspaceEntryAuthorizationOutcome::AwaitingObservation {
            attempted,
            transitioned_grants,
            operation_error,
        } => {
            let observation = observe_after_error(&attempted);
            state.file_authorization().reconcile_rename_after_error(
                attempted,
                transitioned_grants,
                operation_error,
                observation,
            )?
        }
        outcome => outcome,
    };
    match outcome {
        RenameWorkspaceEntryAuthorizationOutcome::ConfirmedNotCommitted { message } => {
            Ok(AuthorizedRenameOutcome::ConfirmedNotCommitted { message })
        }
        RenameWorkspaceEntryAuthorizationOutcome::Committed {
            renamed,
            invalidated_preview_leases,
        } => match invalidate_preview(&invalidated_preview_leases) {
            Ok(()) => Ok(AuthorizedRenameOutcome::Committed(renamed)),
            Err(recovery_message) => Ok(AuthorizedRenameOutcome::RecoveryRequired {
                renamed,
                recovery_message,
            }),
        },
        RenameWorkspaceEntryAuthorizationOutcome::Indeterminate {
            attempted,
            invalidated_preview_leases,
            operation_error,
            observation_message,
        } => {
            let mut recovery_message = format!(
                "Rename may have partially changed the workspace after an error: {operation_error}. Refresh and inspect both paths before retrying."
            );
            recovery_message.push_str(&observation_message);
            if let Err(cleanup_error) = invalidate_preview(&invalidated_preview_leases) {
                recovery_message.push_str(" Preview invalidation also failed: ");
                recovery_message.push_str(&cleanup_error);
            }
            Ok(AuthorizedRenameOutcome::Indeterminate {
                attempted,
                recovery_message,
            })
        }
        RenameWorkspaceEntryAuthorizationOutcome::AwaitingObservation { .. } => {
            unreachable!("rename observation must be reconciled before preview handling")
        }
    }
}

fn rename_authorized_workspace_entry_with_preview_inner(
    state: &AppState,
    workspace_token: &str,
    source_path: impl AsRef<Path>,
    new_name: impl FnOnce(&Path, bool) -> Result<String, String>,
    rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    observe_after_error: impl FnOnce(&RenamedWorkspaceEntry) -> RenameErrorObservation,
    invalidate_preview: impl FnOnce(&HashSet<PreviewLeaseId>) -> Result<(), String>,
) -> Result<AuthorizedRenameOutcome, String> {
    let outcome = state.file_authorization().rename_workspace_entry(
        workspace_token,
        source_path,
        new_name,
        rename,
    )?;
    finish_authorized_workspace_entry_relocation(
        state,
        outcome,
        observe_after_error,
        invalidate_preview,
    )
}

fn move_authorized_workspace_entry_with_preview_inner(
    state: &AppState,
    workspace_token: &str,
    source_path: impl AsRef<Path>,
    destination_parent_path: impl AsRef<Path>,
    rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    observe_after_error: impl FnOnce(&RenamedWorkspaceEntry) -> RenameErrorObservation,
    invalidate_preview: impl FnOnce(&HashSet<PreviewLeaseId>) -> Result<(), String>,
) -> Result<AuthorizedRenameOutcome, String> {
    let outcome = state.file_authorization().move_workspace_entry(
        workspace_token,
        source_path,
        destination_parent_path,
        rename,
    )?;
    finish_authorized_workspace_entry_relocation(
        state,
        outcome,
        observe_after_error,
        invalidate_preview,
    )
}

pub(crate) fn rename_authorized_workspace_entry_inner(
    state: &AppState,
    workspace_token: &str,
    source_path: impl AsRef<Path>,
    new_name: impl FnOnce(&Path, bool) -> Result<String, String>,
    rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    observe_after_error: impl FnOnce(&RenamedWorkspaceEntry) -> RenameErrorObservation,
) -> Result<AuthorizedRenameOutcome, String> {
    rename_authorized_workspace_entry_with_preview_inner(
        state,
        workspace_token,
        source_path,
        new_name,
        rename,
        observe_after_error,
        |invalidated_preview_leases| {
            invalidate_preview_leases_after_authorization(state, invalidated_preview_leases)
        },
    )
}

pub(crate) fn move_authorized_workspace_entry_inner(
    state: &AppState,
    workspace_token: &str,
    source_path: impl AsRef<Path>,
    destination_parent_path: impl AsRef<Path>,
    rename: impl FnOnce(&Path, &Path) -> Result<(), String>,
    observe_after_error: impl FnOnce(&RenamedWorkspaceEntry) -> RenameErrorObservation,
) -> Result<AuthorizedRenameOutcome, String> {
    move_authorized_workspace_entry_with_preview_inner(
        state,
        workspace_token,
        source_path,
        destination_parent_path,
        rename,
        observe_after_error,
        |invalidated_preview_leases| {
            invalidate_preview_leases_after_authorization(state, invalidated_preview_leases)
        },
    )
}

fn delete_authorized_workspace_entry_with_preview_inner(
    state: &AppState,
    workspace_token: &str,
    source_path: impl AsRef<Path>,
    delete: impl FnOnce(&Path, bool) -> Result<(), String>,
    observe_file_after_error: impl FnOnce(&Path) -> Result<DeleteFileObservation, String>,
    invalidate_preview: impl FnOnce(&HashSet<PreviewLeaseId>) -> Result<(), String>,
) -> Result<AuthorizedDeleteOutcome, String> {
    let outcome = state.file_authorization().delete_workspace_entry(
        workspace_token,
        source_path,
        delete,
        observe_file_after_error,
    )?;
    let (deleted, invalidated_preview_leases) = match outcome {
        DeleteWorkspaceEntryAuthorizationOutcome::ConfirmedNotCommitted { message } => {
            return Ok(AuthorizedDeleteOutcome::ConfirmedNotCommitted { message });
        }
        DeleteWorkspaceEntryAuthorizationOutcome::Committed {
            deleted,
            invalidated_preview_leases,
        } => (deleted, invalidated_preview_leases),
        DeleteWorkspaceEntryAuthorizationOutcome::Indeterminate {
            attempted,
            invalidated_preview_leases,
            operation_error,
        } => {
            let (subject, location) = if attempted.is_file() {
                ("File", "file")
            } else {
                ("Directory", "directory")
            };
            let mut recovery_message = format!(
                "{subject} deletion may have partially changed the workspace after an error: {operation_error}. Refresh and inspect the {location} before retrying."
            );
            if let Err(cleanup_error) = invalidate_preview(&invalidated_preview_leases) {
                recovery_message.push_str(" Preview invalidation also failed: ");
                recovery_message.push_str(&cleanup_error);
            }
            return Ok(AuthorizedDeleteOutcome::Indeterminate {
                attempted,
                recovery_message,
            });
        }
    };
    match invalidate_preview(&invalidated_preview_leases) {
        Ok(()) => Ok(AuthorizedDeleteOutcome::Committed(deleted)),
        Err(recovery_message) => Ok(AuthorizedDeleteOutcome::RecoveryRequired {
            deleted,
            recovery_message,
        }),
    }
}

pub(crate) fn delete_authorized_workspace_entry_inner(
    state: &AppState,
    workspace_token: &str,
    source_path: impl AsRef<Path>,
    delete: impl FnOnce(&Path, bool) -> Result<(), String>,
    observe_file_after_error: impl FnOnce(&Path) -> Result<DeleteFileObservation, String>,
) -> Result<AuthorizedDeleteOutcome, String> {
    delete_authorized_workspace_entry_with_preview_inner(
        state,
        workspace_token,
        source_path,
        delete,
        observe_file_after_error,
        |invalidated_preview_leases| {
            invalidate_preview_leases_after_authorization(state, invalidated_preview_leases)
        },
    )
}

pub(crate) fn relocate_authorized_path_prefix_inner(
    state: &AppState,
    old_prefix: &Path,
    new_prefix: &Path,
) -> Result<(), String> {
    apply_authorization_then_preview_invalidation(
        || {
            state
                .file_authorization()
                .relocate_path_prefix(old_prefix, new_prefix)
        },
        |invalidated_preview_leases| {
            invalidate_preview_leases_after_authorization(state, &invalidated_preview_leases)
        },
    )
}

pub(crate) fn commit_indeterminate_delete_inner(
    state: &AppState,
    deleted_path: &Path,
) -> Result<(), String> {
    let mut authorization = state.file_authorization().lock()?;
    authorization.revoke_path_prefix(deleted_path);
    Ok(())
}

pub(crate) fn apply_authorization_then_preview_invalidation<T, R>(
    authorization: impl FnOnce() -> Result<T, String>,
    preview: impl FnOnce(T) -> Result<R, String>,
) -> Result<R, String> {
    let authorized = authorization()?;
    preview(authorized)
}

pub(crate) fn revoke_authorized_path_prefix_inner(
    state: &AppState,
    prefix: &Path,
) -> Result<(), String> {
    apply_authorization_then_preview_invalidation(
        || state.file_authorization().revoke_path_prefix(prefix),
        |invalidated_preview_leases| match state
            .html_preview_server
            .invalidate_preview_leases(&invalidated_preview_leases)
        {
            Ok(()) => Ok(()),
            Err(recovery) => {
                let (error, drained_leases) = recovery.into_parts();
                retire_preview_leases_inner(state, &drained_leases)
                    .map_err(PreviewRetirementError::into_message)?;
                Err(error)
            }
        },
    )
}

#[cfg(test)]
pub(crate) fn revoke_authorized_file_inner(
    state: &AppState,
    file: &AuthorizedFile,
) -> Result<(), String> {
    apply_authorization_then_preview_invalidation(
        || state.file_authorization().revoke_authorized_file(file),
        |invalidated_preview_leases| match state
            .html_preview_server
            .invalidate_preview_leases(&invalidated_preview_leases)
        {
            Ok(()) => Ok(()),
            Err(recovery) => {
                let (error, drained_leases) = recovery.into_parts();
                retire_preview_leases_inner(state, &drained_leases)
                    .map_err(PreviewRetirementError::into_message)?;
                Err(error)
            }
        },
    )
}

pub(crate) fn retire_preview_lease_inner(
    state: &AppState,
    lease: &PreviewLeaseId,
) -> Result<(), PreviewRetirementError> {
    retire_preview_leases_inner(state, &HashSet::from([lease.clone()]))
}

pub(crate) fn retire_preview_leases_inner(
    state: &AppState,
    leases: &HashSet<PreviewLeaseId>,
) -> Result<(), PreviewRetirementError> {
    state.file_authorization().retire_preview_leases(leases)
}

#[cfg(test)]
pub(crate) fn authorize_directory_root_inner(
    state: &AppState,
    root: PathBuf,
) -> Result<PathBuf, String> {
    state
        .file_authorization()
        .authorize_directory_root(root)
        .map(AuthorizedWorkspace::into_root)
}

#[cfg(test)]
pub(crate) fn authorize_file_inner(state: &AppState, file: PathBuf) -> Result<PathBuf, String> {
    state
        .file_authorization()
        .authorize_file(file)
        .map(AuthorizedFile::into_path)
}

#[cfg(test)]
pub(crate) fn authorize_saved_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    state
        .file_authorization()
        .authorize_save_destination(path)
        .map(AuthorizedFile::into_path)
}

#[cfg(test)]
pub(crate) fn authorize_workspace_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    state
        .file_authorization()
        .open_workspace_file(path)
        .map(AuthorizedFile::into_path)
}

pub(crate) fn ensure_authorized_existing_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    state.file_authorization().file_for_read(path)
}

pub(crate) fn ensure_authorized_watch_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    state.file_authorization().file_for_watch(path)
}

#[cfg(test)]
pub(crate) fn ensure_authorized_write_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    state.file_authorization().file_for_write(path)
}

pub(crate) fn ensure_authorized_directory_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<PathBuf, String> {
    state.file_authorization().directory_for_read(path)
}

pub(crate) fn resolve_authorized_workspace_directory_for_token_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
) -> Result<(PathBuf, AuthorizedWorkspace), String> {
    state
        .file_authorization()
        .authorized_workspace_directory_for_token(workspace_token, path)
}

pub(crate) fn resolve_authorized_workspace_root_for_token_inner(
    state: &AppState,
    workspace_token: &str,
    root: impl AsRef<Path>,
) -> Result<AuthorizedWorkspace, String> {
    state
        .file_authorization()
        .authorized_workspace_root_for_token(workspace_token, root)
}

pub(crate) fn is_authorized_image_path(state: &AppState, canonical: &Path) -> Result<bool, String> {
    state.file_authorization().is_authorized_image(canonical)
}

pub(crate) fn preview_scope_for_file_inner(
    state: &AppState,
    file: impl AsRef<Path>,
) -> Result<AuthorizedPreviewScope, String> {
    state.file_authorization().preview_scope_for(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_preview_server::prepare_html_preview_inner;
    use tempfile::tempdir;

    #[test]
    fn normalize_new_path_rejects_parent_components() {
        let dir = tempdir().unwrap();
        assert!(normalize_parent_for_new_path(dir.path().join("ok.md")).is_ok());
        assert!(normalize_parent_for_new_path(dir.path().join("../bad.md")).is_err());
    }

    #[test]
    fn file_only_authorization_denies_sibling_read_and_write() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("allowed.md");
        let sibling = dir.path().join("sibling.md");
        fs::write(&allowed, "# allowed").unwrap();
        fs::write(&sibling, "# sibling").unwrap();
        let state = AppState::default();

        let allowed_canonical = authorize_file_inner(&state, allowed).unwrap();
        assert_eq!(
            ensure_authorized_existing_file_inner(&state, &allowed_canonical).unwrap(),
            allowed_canonical
        );
        assert!(ensure_authorized_existing_file_inner(&state, &sibling).is_err());
        assert!(ensure_authorized_write_file_inner(&state, &sibling).is_err());
    }

    #[test]
    fn prepared_open_document_grant_publishes_only_on_terminal_apply() {
        let directory = tempdir().unwrap();
        let document = directory.path().join("document.md");
        let sibling = directory.path().join("sibling.md");
        fs::write(&document, "# document").unwrap();
        fs::write(&sibling, "# sibling").unwrap();
        let canonical_document = normalize_existing_path(&document).unwrap();
        let canonical_parent = canonical_document.parent().unwrap().to_path_buf();
        let state = AppState::default();

        let error = state
            .file_authorization()
            .with_prepared_open_document_grant(&canonical_document, |_prepared| {
                Err::<(), _>("injected pre-commit failure".to_string())
            })
            .unwrap_err();
        assert_eq!(error, "injected pre-commit failure");
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_document)
                .unwrap(),
            None
        );
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_parent)
                .unwrap(),
            None
        );

        state
            .file_authorization()
            .with_prepared_open_document_grant(&canonical_document, |prepared| {
                prepared.apply();
                Ok(())
            })
            .unwrap();

        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_document)
                .unwrap(),
            Some((GrantStatus::Active, 1))
        );
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_parent)
                .unwrap(),
            Some((GrantStatus::Active, 1))
        );
        assert_eq!(
            ensure_authorized_existing_file_inner(&state, &canonical_document).unwrap(),
            canonical_document
        );
        assert!(ensure_authorized_existing_file_inner(&state, &sibling).is_err());
    }

    #[test]
    fn directory_authorization_allows_descendant_read_but_not_write_without_file_authorization() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("notes");
        fs::create_dir(&nested).unwrap();
        let doc = nested.join("doc.md");
        let new_doc = nested.join("new.md");
        fs::write(&doc, "# doc").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        assert!(ensure_authorized_existing_file_inner(&state, &doc).is_ok());
        assert!(ensure_authorized_write_file_inner(&state, &doc).is_err());
        assert!(ensure_authorized_write_file_inner(&state, &new_doc).is_err());
    }

    #[test]
    fn failed_directory_authorization_grants_no_partial_capability() {
        let workspace = tempdir().unwrap();
        let document = workspace.path().join("document.md");
        fs::write(&document, "# document").unwrap();
        let state = AppState::default();

        let result = state
            .file_authorization()
            .authorize_directory_root_with(workspace.path(), |_| {
                Err("injected authorization commit failure".into())
            });

        assert!(result.is_err());
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_err());
        assert!(!is_authorized_image_path(&state, &document).unwrap());
    }

    #[test]
    fn overlapping_origins_are_reference_counted() {
        let directory = tempdir().unwrap();
        let document = directory.path().join("document.md");
        let asset = directory.path().join("asset.png");
        fs::write(&document, "# document").unwrap();
        fs::write(&asset, b"png").unwrap();
        let canonical_asset = normalize_existing_path(&asset).unwrap();
        let state = AppState::default();

        let first_open = state
            .file_authorization()
            .authorize_file(&document)
            .unwrap();
        let second_open = state
            .file_authorization()
            .authorize_file(&document)
            .unwrap();
        let save = state
            .file_authorization()
            .authorize_save_destination(&document)
            .unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(first_open.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(second_open.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(save.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_asset).unwrap());
    }

    #[test]
    fn mixed_standalone_and_workspace_acquisitions_revoke_independently() {
        let workspace_root = tempdir().unwrap();
        let document = workspace_root.path().join("document.md");
        let asset = workspace_root.path().join("asset.png");
        fs::write(&document, "# document").unwrap();
        fs::write(&asset, b"png").unwrap();
        let canonical_root = normalize_existing_path(workspace_root.path()).unwrap();
        let canonical_asset = normalize_existing_path(&asset).unwrap();
        let state = AppState::default();

        let authorized_workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_root)
            .unwrap();
        let standalone = state
            .file_authorization()
            .authorize_file(&document)
            .unwrap();
        let workspace = state
            .file_authorization()
            .open_workspace_file(&document)
            .unwrap();

        assert_ne!(standalone.origin(), workspace.origin());

        let workspace_root_origin = GrantOrigin::Workspace(*authorized_workspace.token());
        state
            .file_authorization()
            .revoke_origin(&workspace_root_origin, RevokeOriginMode::All)
            .unwrap();
        state
            .file_authorization()
            .revoke_origin(workspace.origin(), RevokeOriginMode::All)
            .unwrap();

        assert!(ensure_authorized_write_file_inner(&state, &document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(standalone.origin(), RevokeOriginMode::All)
            .unwrap();

        assert!(ensure_authorized_write_file_inner(&state, &document).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_asset).unwrap());
    }

    #[test]
    fn mixed_standalone_and_workspace_acquisitions_revoke_independently_in_inverse_order() {
        let workspace_root = tempdir().unwrap();
        let document = workspace_root.path().join("document.md");
        let asset = workspace_root.path().join("asset.png");
        fs::write(&document, "# document").unwrap();
        fs::write(&asset, b"png").unwrap();
        let canonical_root = normalize_existing_path(workspace_root.path()).unwrap();
        let canonical_asset = normalize_existing_path(&asset).unwrap();
        let state = AppState::default();

        let authorized_workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_root)
            .unwrap();
        let standalone = state
            .file_authorization()
            .authorize_file(&document)
            .unwrap();
        let workspace = state
            .file_authorization()
            .open_workspace_file(&document)
            .unwrap();
        let workspace_root_origin = GrantOrigin::Workspace(*authorized_workspace.token());

        assert_ne!(standalone.origin(), workspace.origin());
        assert_ne!(standalone.origin(), &workspace_root_origin);
        assert_ne!(workspace.origin(), &workspace_root_origin);

        state
            .file_authorization()
            .revoke_origin(standalone.origin(), RevokeOriginMode::All)
            .unwrap();

        assert!(ensure_authorized_write_file_inner(&state, &document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(workspace.origin(), RevokeOriginMode::All)
            .unwrap();

        assert!(ensure_authorized_write_file_inner(&state, &document).is_err());
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(&workspace_root_origin, RevokeOriginMode::All)
            .unwrap();

        assert!(ensure_authorized_existing_file_inner(&state, &document).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_asset).unwrap());
    }

    #[test]
    fn workspace_entry_uses_explicit_token_not_first_matching_root() {
        let outer = tempdir().unwrap();
        let inner = outer.path().join("inner");
        let document = inner.join("document.md");
        fs::create_dir(&inner).unwrap();
        fs::write(&document, "# document").unwrap();
        let canonical_outer = normalize_existing_path(outer.path()).unwrap();
        let canonical_inner = normalize_existing_path(&inner).unwrap();
        let state = AppState::default();

        let outer_workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_outer)
            .unwrap();
        let inner_workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_inner)
            .unwrap();

        let (_, selected_outer) = state
            .file_authorization()
            .workspace_entry_for_mutation(outer_workspace.token(), &document)
            .unwrap();
        let (_, selected_inner) = state
            .file_authorization()
            .workspace_entry_for_mutation(inner_workspace.token(), &document)
            .unwrap();

        assert_eq!(selected_outer, canonical_outer);
        assert_eq!(selected_inner, canonical_inner);
    }

    #[test]
    fn cannot_select_any_active_workspace_root_as_a_mutation_target_through_an_overlap() {
        let outer = tempdir().unwrap();
        let inner = outer.path().join("inner");
        fs::create_dir(&inner).unwrap();
        let canonical_outer = normalize_existing_path(outer.path()).unwrap();
        let canonical_inner = normalize_existing_path(&inner).unwrap();
        let state = AppState::default();

        let outer_workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_outer)
            .unwrap();
        state
            .file_authorization()
            .authorize_directory_root(&canonical_inner)
            .unwrap();

        assert!(state
            .file_authorization()
            .workspace_entry_for_mutation(outer_workspace.token(), &canonical_inner)
            .is_err());
    }

    #[test]
    fn rename_transaction_commits_filesystem_and_authorization_before_preview_invalidation() {
        use lock_order_test_probe::{trace, LockEvent};

        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.html");
        fs::write(&source, "draft").unwrap();
        let state = AppState::default();
        let authorized_workspace = state
            .file_authorization()
            .authorize_directory_root(workspace.path())
            .unwrap();
        state.file_authorization().authorize_file(&source).unwrap();
        prepare_html_preview_inner(&state, &source, "draft").unwrap();
        let canonical_source = source.canonicalize().unwrap();
        let target = canonical_source.with_file_name("renamed.html");

        let (outcome, events) = trace(|| {
            rename_authorized_workspace_entry_inner(
                &state,
                &authorized_workspace.wire_token(),
                &canonical_source,
                |entry, is_file| {
                    assert_eq!(entry, canonical_source);
                    assert!(is_file);
                    Ok("renamed.html".to_string())
                },
                |old_path, new_path| {
                    assert_eq!(old_path, canonical_source);
                    assert_eq!(new_path, target);
                    fs::rename(old_path, new_path)
                        .map_err(|err| format!("Failed to rename entry: {err}"))
                },
                |_| panic!("observation must not run after a successful rename"),
            )
        });

        let AuthorizedRenameOutcome::Committed(renamed) = outcome.unwrap() else {
            panic!("expected committed rename outcome");
        };
        assert_eq!(
            renamed.workspace().wire_token(),
            authorized_workspace.wire_token()
        );
        assert_eq!(renamed.old_path(), canonical_source);
        assert_eq!(renamed.new_path(), target);
        assert!(renamed.is_file());
        assert!(!canonical_source.exists());
        assert!(target.is_file());
        assert!(ensure_authorized_write_file_inner(&state, &target).is_ok());
        assert!(ensure_authorized_write_file_inner(&state, &canonical_source).is_err());
        assert_eq!(
            events,
            [
                LockEvent::AuthorizationAcquired,
                LockEvent::AuthorizationReleased,
                LockEvent::HtmlSitesAcquired,
                LockEvent::HtmlSitesReleased,
            ]
        );
    }

    #[test]
    fn rename_transaction_returns_recovery_required_for_post_commit_preview_failure() {
        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.md");
        fs::write(&source, "draft").unwrap();
        let state = AppState::default();
        let authorized_workspace = state
            .file_authorization()
            .authorize_directory_root(workspace.path())
            .unwrap();
        let canonical_source = source.canonicalize().unwrap();
        let target = canonical_source.with_file_name("renamed.md");

        let outcome = rename_authorized_workspace_entry_with_preview_inner(
            &state,
            &authorized_workspace.wire_token(),
            &source,
            |_, is_file| {
                assert!(is_file);
                Ok("renamed.md".to_string())
            },
            |old_path, new_path| {
                fs::rename(old_path, new_path)
                    .map_err(|err| format!("Failed to rename entry: {err}"))
            },
            |_| panic!("observation must not run after a successful rename"),
            |_| Err("injected post-commit preview failure".to_string()),
        )
        .unwrap();

        let AuthorizedRenameOutcome::RecoveryRequired {
            renamed,
            recovery_message,
        } = outcome
        else {
            panic!("expected recovery-required rename outcome");
        };
        assert_eq!(renamed.old_path(), canonical_source);
        assert_eq!(renamed.new_path(), target);
        assert!(target.is_file());
        assert_eq!(recovery_message, "injected post-commit preview failure");
    }

    #[test]
    fn rename_transaction_rejects_a_valid_token_for_another_workspace_before_commit() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let source = second.path().join("draft.md");
        fs::write(&source, "draft").unwrap();
        let state = AppState::default();
        let first_workspace = state
            .file_authorization()
            .authorize_directory_root(first.path())
            .unwrap();
        state
            .file_authorization()
            .authorize_directory_root(second.path())
            .unwrap();
        let rename_calls = std::cell::Cell::new(0);

        let error = match rename_authorized_workspace_entry_inner(
            &state,
            &first_workspace.wire_token(),
            &source,
            |_, _| Ok("renamed.md".to_string()),
            |_, _| {
                rename_calls.set(rename_calls.get() + 1);
                Ok(())
            },
            |_| panic!("observation must not run before filesystem mutation"),
        ) {
            Ok(_) => panic!("wrong-workspace rename must fail before commit"),
            Err(error) => error,
        };

        assert_eq!(error, "Workspace entry is outside the selected workspace");
        assert_eq!(rename_calls.get(), 0);
        assert!(source.is_file());
        assert!(!second.path().join("renamed.md").exists());
    }

    #[test]
    fn relocate_prefix_relocates_every_descendant_capability_and_revokes_old_prefix() {
        let container = tempdir().unwrap();
        let old_root = container.path().join("old");
        let old_document = old_root.join("nested/document.md");
        let old_asset = old_root.join("nested/asset.png");
        fs::create_dir_all(old_document.parent().unwrap()).unwrap();
        fs::write(&old_document, "# document").unwrap();
        fs::write(&old_asset, b"png").unwrap();
        let canonical_old_root = normalize_existing_path(&old_root).unwrap();
        let state = AppState::default();
        let workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_old_root)
            .unwrap();
        state
            .file_authorization()
            .authorize_file(&old_document)
            .unwrap();

        let new_root = container.path().join("new");
        fs::rename(&old_root, &new_root).unwrap();
        let canonical_new_root = normalize_existing_path(&new_root).unwrap();
        let new_document = canonical_new_root.join("nested/document.md");
        let new_asset = canonical_new_root.join("nested/asset.png");
        state
            .file_authorization()
            .relocate_path_prefix(&canonical_old_root, &canonical_new_root)
            .unwrap();

        assert!(state
            .file_authorization()
            .workspace_entry_for_mutation(workspace.token(), &new_document)
            .is_ok());
        assert!(ensure_authorized_existing_file_inner(&state, &new_document).is_ok());
        assert!(ensure_authorized_write_file_inner(&state, &new_document).is_ok());
        assert!(
            is_authorized_image_path(&state, &normalize_existing_path(&new_asset).unwrap())
                .unwrap()
        );

        fs::create_dir_all(old_document.parent().unwrap()).unwrap();
        fs::write(&old_document, "# recreated").unwrap();
        fs::write(&old_asset, b"recreated").unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &old_document).is_err());
        assert!(ensure_authorized_write_file_inner(&state, &old_document).is_err());
        assert!(
            !is_authorized_image_path(&state, &normalize_existing_path(&old_asset).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn revoke_prefix_purges_exact_and_descendant_capabilities() {
        let directory = tempdir().unwrap();
        let retained_directory = tempdir().unwrap();
        let revoked_root = directory.path().join("revoked");
        let nested = revoked_root.join("nested");
        let first_document = revoked_root.join("first.md");
        let second_document = nested.join("second.md");
        let first_asset = revoked_root.join("first.png");
        let second_asset = nested.join("second.png");
        let retained_document = retained_directory.path().join("retained.md");
        fs::create_dir_all(&nested).unwrap();
        fs::write(&first_document, "# first").unwrap();
        fs::write(&second_document, "# second").unwrap();
        fs::write(&first_asset, b"first").unwrap();
        fs::write(&second_asset, b"second").unwrap();
        fs::write(&retained_document, "# retained").unwrap();
        let canonical_revoked_root = normalize_existing_path(&revoked_root).unwrap();
        let canonical_first_asset = normalize_existing_path(&first_asset).unwrap();
        let canonical_second_asset = normalize_existing_path(&second_asset).unwrap();
        let state = AppState::default();
        state
            .file_authorization()
            .authorize_file(&first_document)
            .unwrap();
        state
            .file_authorization()
            .authorize_file(&second_document)
            .unwrap();
        state
            .file_authorization()
            .authorize_file(&retained_document)
            .unwrap();

        state
            .file_authorization()
            .revoke_path_prefix(&canonical_revoked_root)
            .unwrap();

        assert!(ensure_authorized_existing_file_inner(&state, &first_document).is_err());
        assert!(ensure_authorized_write_file_inner(&state, &first_document).is_err());
        assert!(ensure_authorized_existing_file_inner(&state, &second_document).is_err());
        assert!(ensure_authorized_write_file_inner(&state, &second_document).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_first_asset).unwrap());
        assert!(!is_authorized_image_path(&state, &canonical_second_asset).unwrap());
        assert!(ensure_authorized_write_file_inner(&state, &retained_document).is_ok());
    }

    #[test]
    fn relocate_prefix_preserves_all_origins_without_collapsing_provenance() {
        let container = tempdir().unwrap();
        let old_root = container.path().join("old");
        let new_root = container.path().join("new");
        let old_document = old_root.join("document.md");
        let new_document = new_root.join("document.md");
        let new_asset = new_root.join("asset.png");
        fs::create_dir_all(&old_root).unwrap();
        fs::create_dir_all(&new_root).unwrap();
        fs::write(&old_document, "# old").unwrap();
        fs::write(&new_document, "# new").unwrap();
        fs::write(&new_asset, b"png").unwrap();
        let canonical_old_root = normalize_existing_path(&old_root).unwrap();
        let canonical_new_root = normalize_existing_path(&new_root).unwrap();
        let canonical_new_asset = normalize_existing_path(&new_asset).unwrap();
        let state = AppState::default();

        let source_open = state
            .file_authorization()
            .authorize_file(&old_document)
            .unwrap();
        let source_save = state
            .file_authorization()
            .authorize_save_destination(&old_document)
            .unwrap();
        let destination_open = state
            .file_authorization()
            .authorize_file(&new_document)
            .unwrap();

        state
            .file_authorization()
            .relocate_path_prefix(&canonical_old_root, &canonical_new_root)
            .unwrap();

        state
            .file_authorization()
            .revoke_origin(source_open.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(ensure_authorized_write_file_inner(&state, &new_document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_new_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(destination_open.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(ensure_authorized_write_file_inner(&state, &new_document).is_ok());
        assert!(is_authorized_image_path(&state, &canonical_new_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(source_save.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(ensure_authorized_write_file_inner(&state, &new_document).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_new_asset).unwrap());
    }

    #[test]
    fn relocate_prefix_does_not_reactivate_suspended_collision_origins() {
        let old_root = PathBuf::from("/workspace/old");
        let new_root = PathBuf::from("/workspace/new");
        let old_key = GrantKey::ExactReadWrite(old_root.join("document.md"));
        let new_key = GrantKey::ExactReadWrite(new_root.join("document.md"));
        let active_origin = GrantOrigin::OpenDocument(DocumentGrantId(1));
        let suspended_origin = GrantOrigin::SaveAs(DocumentGrantId(2));
        let mut state = AuthorizationState::default();
        state.grant(old_key, active_origin.clone());
        state.grant(new_key.clone(), suspended_origin.clone());
        state.grants.get_mut(&new_key).unwrap().suspend();

        state.relocate_path_prefix(&old_root, &new_root);

        let relocated = state.grants.get(&new_key).unwrap();
        assert_eq!(relocated.status, GrantStatus::Active);
        assert_eq!(relocated.origins, HashMap::from([(active_origin, 1)]));
        assert!(!relocated.origins.contains_key(&suspended_origin));
    }

    #[test]
    fn revoke_prefix_preserves_ancestor_workspace_grant() {
        let workspace = tempdir().unwrap();
        let document = workspace.path().join("document.md");
        let sibling = workspace.path().join("sibling.md");
        let asset = workspace.path().join("asset.png");
        fs::write(&document, "# document").unwrap();
        fs::write(&sibling, "# sibling").unwrap();
        fs::write(&asset, b"png").unwrap();
        let canonical_root = normalize_existing_path(workspace.path()).unwrap();
        let canonical_document = normalize_existing_path(&document).unwrap();
        let canonical_asset = normalize_existing_path(&asset).unwrap();
        let state = AppState::default();

        let authorized_workspace = state
            .file_authorization()
            .authorize_directory_root(&canonical_root)
            .unwrap();
        state
            .file_authorization()
            .authorize_file(&canonical_document)
            .unwrap();

        state
            .file_authorization()
            .revoke_path_prefix(&canonical_document)
            .unwrap();

        assert!(ensure_authorized_write_file_inner(&state, &canonical_document).is_err());
        assert!(ensure_authorized_existing_file_inner(&state, &canonical_document).is_ok());
        assert!(state
            .file_authorization()
            .workspace_entry_for_mutation(authorized_workspace.token(), &sibling)
            .is_ok());
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());
    }

    #[test]
    fn internal_asset_grants_are_reference_counted_and_revocable_by_origin() {
        let directory = tempdir().unwrap();
        let first_document = directory.path().join("first.html");
        let second_document = directory.path().join("second.html");
        let asset = directory.path().join("asset.png");
        fs::write(&first_document, "first").unwrap();
        fs::write(&second_document, "second").unwrap();
        fs::write(&asset, b"png").unwrap();
        let canonical_asset = normalize_existing_path(&asset).unwrap();
        let state = AppState::default();

        let first_document_grant = state
            .file_authorization()
            .authorize_file(&first_document)
            .unwrap();
        let second_document_grant = state
            .file_authorization()
            .authorize_file(&second_document)
            .unwrap();
        let (_, _, first_preview_lease) = state
            .file_authorization()
            .preview_scope_for(&first_document)
            .unwrap()
            .into_parts();
        let (_, _, second_preview_lease) = state
            .file_authorization()
            .preview_scope_for(&second_document)
            .unwrap()
            .into_parts();

        state
            .file_authorization()
            .revoke_origin(first_document_grant.origin(), RevokeOriginMode::All)
            .unwrap();
        state
            .file_authorization()
            .revoke_origin(second_document_grant.origin(), RevokeOriginMode::All)
            .unwrap();
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(
                &GrantOrigin::Preview(first_preview_lease),
                RevokeOriginMode::All,
            )
            .unwrap();
        assert!(is_authorized_image_path(&state, &canonical_asset).unwrap());

        state
            .file_authorization()
            .revoke_origin(
                &GrantOrigin::Preview(second_preview_lease),
                RevokeOriginMode::All,
            )
            .unwrap();
        assert!(!is_authorized_image_path(&state, &canonical_asset).unwrap());
    }

    #[test]
    fn revoke_prefix_and_origin_revocation_invalidate_unsupported_preview_sites() {
        let workspace = tempdir().unwrap();
        let removed_root = workspace.path().join("removed");
        fs::create_dir(&removed_root).unwrap();
        let removed_document = removed_root.join("index.html");
        let retained_document = workspace.path().join("retained.html");
        fs::write(&removed_document, "removed").unwrap();
        fs::write(&retained_document, "retained").unwrap();
        let canonical_removed_root = normalize_existing_path(&removed_root).unwrap();
        let canonical_removed_document = normalize_existing_path(&removed_document).unwrap();
        let canonical_retained_document = normalize_existing_path(&retained_document).unwrap();
        let state = AppState::default();
        state
            .file_authorization()
            .authorize_directory_root(workspace.path())
            .unwrap();
        prepare_html_preview_inner(&state, &removed_document, "removed").unwrap();
        prepare_html_preview_inner(&state, &retained_document, "retained").unwrap();

        fs::remove_dir_all(&removed_root).unwrap();
        revoke_authorized_path_prefix_inner(&state, &canonical_removed_root).unwrap();

        let documents = state.html_preview_server.site_documents().unwrap();
        assert!(!documents.contains(&canonical_removed_document));
        assert!(documents.contains(&canonical_retained_document));

        let standalone = tempdir().unwrap();
        let standalone_document = standalone.path().join("standalone.html");
        let standalone_asset = standalone.path().join("asset.png");
        fs::write(&standalone_document, "standalone").unwrap();
        fs::write(&standalone_asset, b"png").unwrap();
        let canonical_standalone_document = normalize_existing_path(&standalone_document).unwrap();
        let canonical_standalone_asset = normalize_existing_path(&standalone_asset).unwrap();
        let authorized_file = state
            .file_authorization()
            .authorize_file(&standalone_document)
            .unwrap();
        prepare_html_preview_inner(&state, &standalone_document, "standalone").unwrap();

        revoke_authorized_file_inner(&state, &authorized_file).unwrap();

        assert!(!state
            .html_preview_server
            .site_documents()
            .unwrap()
            .contains(&canonical_standalone_document));
        assert!(!is_authorized_image_path(&state, &canonical_standalone_asset).unwrap());
    }

    #[test]
    fn authorization_and_preview_operations_obey_one_lock_order() {
        use lock_order_test_probe::{trace, LockEvent};

        fn assert_authorization_then_preview<T>(operation: impl FnOnce() -> Result<T, String>) {
            let (result, events) = trace(operation);
            result.unwrap();
            assert_eq!(
                events,
                [
                    LockEvent::AuthorizationAcquired,
                    LockEvent::AuthorizationReleased,
                    LockEvent::HtmlSitesAcquired,
                    LockEvent::HtmlSitesReleased,
                ]
            );
        }

        let prepare_dir = tempdir().unwrap();
        let prepare_document = prepare_dir.path().join("prepare.html");
        fs::write(&prepare_document, "prepare").unwrap();
        let prepare_state = AppState::default();
        prepare_state
            .file_authorization()
            .authorize_file(&prepare_document)
            .unwrap();
        assert_authorization_then_preview(|| {
            prepare_html_preview_inner(&prepare_state, &prepare_document, "prepare")
        });

        let relocate_dir = tempdir().unwrap();
        let old_root = relocate_dir.path().join("old");
        fs::create_dir(&old_root).unwrap();
        let old_document = old_root.join("relocate.html");
        fs::write(&old_document, "relocate").unwrap();
        let canonical_old_root = normalize_existing_path(&old_root).unwrap();
        let relocate_state = AppState::default();
        relocate_state
            .file_authorization()
            .authorize_file(&old_document)
            .unwrap();
        prepare_html_preview_inner(&relocate_state, &old_document, "relocate").unwrap();
        let new_root = relocate_dir.path().join("new");
        fs::rename(&old_root, &new_root).unwrap();
        let canonical_new_root = normalize_existing_path(&new_root).unwrap();
        assert_authorization_then_preview(|| {
            relocate_authorized_path_prefix_inner(
                &relocate_state,
                &canonical_old_root,
                &canonical_new_root,
            )
        });

        let revoke_prefix_dir = tempdir().unwrap();
        let revoke_prefix_root = revoke_prefix_dir.path().join("workspace");
        fs::create_dir(&revoke_prefix_root).unwrap();
        let revoke_prefix_document = revoke_prefix_root.join("revoke-prefix.html");
        fs::write(&revoke_prefix_document, "revoke prefix").unwrap();
        let canonical_revoke_prefix = normalize_existing_path(&revoke_prefix_root).unwrap();
        let revoke_prefix_state = AppState::default();
        revoke_prefix_state
            .file_authorization()
            .authorize_directory_root(&revoke_prefix_root)
            .unwrap();
        prepare_html_preview_inner(
            &revoke_prefix_state,
            &revoke_prefix_document,
            "revoke prefix",
        )
        .unwrap();
        assert_authorization_then_preview(|| {
            revoke_authorized_path_prefix_inner(&revoke_prefix_state, &canonical_revoke_prefix)
        });

        let revoke_file_dir = tempdir().unwrap();
        let revoke_file_document = revoke_file_dir.path().join("revoke-file.html");
        fs::write(&revoke_file_document, "revoke file").unwrap();
        let revoke_file_state = AppState::default();
        let authorized_file = revoke_file_state
            .file_authorization()
            .authorize_file(&revoke_file_document)
            .unwrap();
        prepare_html_preview_inner(&revoke_file_state, &revoke_file_document, "revoke file")
            .unwrap();
        assert_authorization_then_preview(|| {
            revoke_authorized_file_inner(&revoke_file_state, &authorized_file)
        });
    }

    #[test]
    fn stale_invalidation_cannot_remove_reprepared_same_path_site() {
        use std::{
            io::{Read, Write},
            net::TcpStream,
        };

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("index.html");
        fs::write(&document, "old generation").unwrap();
        let canonical_document = normalize_existing_path(&document).unwrap();
        let state = AppState::default();
        state
            .file_authorization()
            .authorize_directory_root(workspace.path())
            .unwrap();
        let old_url = prepare_html_preview_inner(&state, &document, "old generation").unwrap();

        let stale_leases = state
            .file_authorization()
            .revoke_path_prefix(&canonical_document)
            .unwrap();
        assert_eq!(stale_leases.len(), 1);

        let (new_url, reprepare_events) = lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &document, "new generation")
        });
        let new_url = new_url.unwrap();
        state
            .html_preview_server
            .invalidate_preview_leases(&stale_leases)
            .unwrap();

        assert!(
            state
                .html_preview_server
                .site_documents()
                .unwrap()
                .contains(&canonical_document),
            "stale invalidation removed the re-prepared same-path site"
        );
        assert_eq!(new_url, old_url);
        let address_and_path = new_url.strip_prefix("http://").unwrap();
        let (address, path) = address_and_path.split_once('/').unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "GET /{path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("new generation"));
        assert_eq!(
            reprepare_events,
            [
                lock_order_test_probe::LockEvent::AuthorizationAcquired,
                lock_order_test_probe::LockEvent::AuthorizationReleased,
                lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                lock_order_test_probe::LockEvent::HtmlSitesReleased,
                lock_order_test_probe::LockEvent::AuthorizationAcquired,
                lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn directory_snapshot_failure_consumes_candidate_and_publishes_nothing() {
        use std::cell::Cell;

        let workspace = tempdir().unwrap();
        let session = FileAuthorizationSession::default();
        let snapshot_calls = Cell::new(0);
        let transport_calls = Cell::new(0);

        let result = session.open_workspace(
            workspace.path(),
            |source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                let WorkspaceSnapshotSource::Candidate(candidate) = source else {
                    panic!("initial workspace snapshot must receive a candidate");
                };
                assert_eq!(candidate.root, workspace.path().canonicalize().unwrap());
                Err::<(), _>("injected first snapshot failure".to_string())
            },
            |_| {
                transport_calls.set(transport_calls.get() + 1);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert_eq!(snapshot_calls.get(), 1);
        assert_eq!(transport_calls.get(), 0);
        let state = session.lock().unwrap();
        assert!(state.workspaces.is_empty());
        assert!(state.grants.is_empty());
        assert_eq!(state.next_workspace_token_id, 0);
    }

    #[test]
    fn persisted_workspace_restore_rejects_a_retargeted_root_before_snapshot_or_grant() {
        use std::cell::Cell;

        let expected = tempdir().unwrap();
        let retargeted = tempdir().unwrap();
        let expected_root = expected.path().canonicalize().unwrap();
        let session = FileAuthorizationSession::default();
        let snapshot_calls = Cell::new(0);
        let transport_calls = Cell::new(0);

        let result = session.open_workspace_at_canonical_root(
            retargeted.path(),
            &expected_root,
            |_| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Ok(())
            },
            |_| {
                transport_calls.set(transport_calls.get() + 1);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(transport_calls.get(), 0);
        let state = session.lock().unwrap();
        assert!(state.workspaces.is_empty());
        assert!(state.grants.is_empty());
    }

    #[test]
    fn directory_transport_failure_publishes_no_workspace_token_or_application_grant() {
        use std::cell::Cell;

        let workspace = tempdir().unwrap();
        let canonical_workspace = workspace.path().canonicalize().unwrap();
        let session = FileAuthorizationSession::default();
        let snapshot_calls = Cell::new(0);
        let transport_calls = Cell::new(0);

        let result = session.open_workspace(
            workspace.path(),
            |source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                let WorkspaceSnapshotSource::Candidate(candidate) = source else {
                    panic!("initial workspace snapshot must receive a candidate");
                };
                assert_eq!(candidate.root, canonical_workspace);
                Ok(())
            },
            |root| {
                transport_calls.set(transport_calls.get() + 1);
                assert_eq!(root, canonical_workspace);
                Err("injected directory transport failure".to_string())
            },
        );
        let error = match result {
            Ok(_) => panic!("transport failure must prevent workspace publication"),
            Err(error) => error,
        };

        assert_eq!(error, "injected directory transport failure");
        assert_eq!(snapshot_calls.get(), 1);
        assert_eq!(transport_calls.get(), 1);
        let state = session.lock().unwrap();
        assert!(state.workspaces.is_empty());
        assert!(state.grants.is_empty());
        assert_eq!(state.next_workspace_token_id, 0);
    }

    #[test]
    fn monotonic_transport_is_not_treated_as_application_authorization() {
        use std::cell::RefCell;

        let directory = tempdir().unwrap();
        let document = directory.path().join("index.html");
        let asset = directory.path().join("asset.png");
        fs::write(&document, "before").unwrap();
        fs::write(&asset, b"png").unwrap();
        let state = AppState::default();
        let transport_roots = RefCell::new(HashSet::new());

        let result = state.file_authorization().open_standalone_file(
            &document,
            |_| Ok(()),
            |parent| {
                transport_roots.borrow_mut().insert(parent.to_path_buf());
                Err("transport reported failure after allowing parent".to_string())
            },
        );
        assert!(result.is_err());

        fs::remove_file(&document).unwrap();
        fs::write(&document, "recreated").unwrap();
        let canonical_document = normalize_existing_path(&document).unwrap();
        let canonical_asset = normalize_existing_path(&asset).unwrap();
        assert!(transport_roots
            .borrow()
            .iter()
            .any(|root| path_is_under(&canonical_document, root)));
        assert!(ensure_authorized_existing_file_inner(&state, &canonical_document).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_asset).unwrap());
        assert!(prepare_html_preview_inner(&state, &canonical_document, "recreated").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn directory_authorization_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_doc = outside.path().join("outside.md");
        let linked_doc = workspace.path().join("linked.md");
        fs::write(&outside_doc, "# outside").unwrap();
        symlink(&outside_doc, &linked_doc).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        assert!(ensure_authorized_existing_file_inner(&state, &linked_doc).is_err());
        assert!(ensure_authorized_write_file_inner(&state, &linked_doc).is_err());
    }

    #[test]
    fn save_as_exact_path_allows_later_write_only_for_that_path() {
        let dir = tempdir().unwrap();
        let saved = dir.path().join("saved.md");
        let sibling = dir.path().join("sibling.md");
        fs::write(&sibling, "# sibling").unwrap();
        let state = AppState::default();
        let authorized = authorize_saved_file_inner(&state, &saved).unwrap();

        assert_eq!(
            ensure_authorized_write_file_inner(&state, &saved).unwrap(),
            authorized
        );
        assert!(ensure_authorized_write_file_inner(&state, &sibling).is_err());
    }

    #[test]
    fn confirmed_external_missing_revokes_exact_grant_and_preview_but_allows_reauthorization() {
        let directory = tempdir().unwrap();
        let document = directory.path().join("draft.html");
        fs::write(&document, "saved").unwrap();
        let state = AppState::default();
        let canonical = authorize_file_inner(&state, document.clone()).unwrap();
        prepare_html_preview_inner(&state, &canonical, "live draft").unwrap();

        fs::remove_file(&canonical).unwrap();
        assert_eq!(
            ensure_authorized_watch_file_inner(&state, &canonical).unwrap(),
            canonical
        );
        revoke_authorized_path_prefix_inner(&state, &canonical).unwrap();

        assert!(ensure_authorized_watch_file_inner(&state, &canonical).is_err());
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());

        fs::write(&canonical, "recreated").unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &canonical).is_err());
        assert_eq!(
            authorize_file_inner(&state, canonical.clone()).unwrap(),
            canonical
        );
    }

    #[test]
    fn confirmed_external_missing_preserves_ancestor_workspace_authorization() {
        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.md");
        fs::write(&document, "before").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();
        let canonical = authorize_workspace_file_inner(&state, &document).unwrap();

        fs::remove_file(&canonical).unwrap();
        revoke_authorized_path_prefix_inner(&state, &canonical).unwrap();
        fs::write(&canonical, "after").unwrap();

        assert_eq!(
            authorize_workspace_file_inner(&state, &canonical).unwrap(),
            canonical
        );
    }
}
