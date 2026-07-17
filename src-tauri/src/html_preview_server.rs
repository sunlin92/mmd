use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::Duration,
};

use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use tauri::State;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[cfg(test)]
use std::ops::{Deref, DerefMut};

use crate::{
    path_auth::{
        normalize_existing_path, path_is_under, preview_scope_for_file_inner,
        retire_preview_lease_inner, retire_preview_leases_inner, AuthorizedPreviewScope,
        PreviewLeaseId, PreviewRetirementError,
    },
    state::AppState,
    workspace_file_kind::WorkspaceFileKind,
};

const PREVIEW_WORKERS: usize = 4;
const MAX_PREVIEW_SITES: usize = 8;
const POISONED_PREVIEW_SITES_ERROR: &str =
    "HTML preview server state was poisoned; all preview sites were stopped";

#[cfg(test)]
mod site_start_test_probe {
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum Event {
        LoopbackBindAttempted,
        WorkerSpawnAttempted,
    }

    thread_local! {
        static TRACE: RefCell<Option<Vec<Event>>> = const { RefCell::new(None) };
    }

    pub(super) fn trace<T>(operation: impl FnOnce() -> T) -> (T, Vec<Event>) {
        TRACE.with(|trace| {
            assert!(
                trace.borrow().is_none(),
                "site-start traces cannot be nested"
            );
            *trace.borrow_mut() = Some(Vec::new());
        });
        let result = operation();
        let events = TRACE.with(|trace| {
            trace
                .borrow_mut()
                .take()
                .expect("site-start trace is active")
        });
        (result, events)
    }

    fn record(event: Event) {
        TRACE.with(|trace| {
            if let Some(events) = trace.borrow_mut().as_mut() {
                events.push(event);
            }
        });
    }

    pub(super) fn loopback_bind_attempted() {
        record(Event::LoopbackBindAttempted);
    }

    pub(super) fn worker_spawn_attempted() {
        record(Event::WorkerSpawnAttempted);
    }
}

struct HtmlPreviewSite {
    url: String,
    root: PathBuf,
    content: Arc<Mutex<String>>,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    lease: PreviewLeaseId,
}

struct HtmlPreviewCommit {
    document: PathBuf,
    url: String,
    active_lease: PreviewLeaseId,
    retired_leases: HashSet<PreviewLeaseId>,
}

enum HtmlPreviewSiteTransactionError {
    Operation(String),
    SitesRecovery(HtmlPreviewSitesRecoveryError),
}

#[derive(Debug)]
pub(crate) struct HtmlPreviewSitesRecoveryError {
    drained_leases: HashSet<PreviewLeaseId>,
}

impl HtmlPreviewSitesRecoveryError {
    pub(crate) fn into_parts(self) -> (String, HashSet<PreviewLeaseId>) {
        (
            POISONED_PREVIEW_SITES_ERROR.to_string(),
            self.drained_leases,
        )
    }

    pub(crate) fn into_message(self) -> String {
        self.into_parts().0
    }
}

#[derive(Default)]
pub(crate) struct HtmlPreviewServerState {
    sites: Mutex<HashMap<PathBuf, HtmlPreviewSite>>,
    #[cfg(test)]
    next_site_start_error: Mutex<Option<String>>,
}

#[cfg(test)]
struct HtmlPreviewSitesGuard<'a> {
    inner: Option<MutexGuard<'a, HashMap<PathBuf, HtmlPreviewSite>>>,
}

#[cfg(not(test))]
type HtmlPreviewSitesGuard<'a> = MutexGuard<'a, HashMap<PathBuf, HtmlPreviewSite>>;

#[cfg(test)]
impl<'a> HtmlPreviewSitesGuard<'a> {
    fn new(inner: MutexGuard<'a, HashMap<PathBuf, HtmlPreviewSite>>) -> Self {
        crate::path_auth::lock_order_test_probe::html_sites_acquired();
        Self { inner: Some(inner) }
    }
}

#[cfg(test)]
impl Deref for HtmlPreviewSitesGuard<'_> {
    type Target = HashMap<PathBuf, HtmlPreviewSite>;

    fn deref(&self) -> &Self::Target {
        self.inner.as_deref().expect("HTML sites guard is active")
    }
}

#[cfg(test)]
impl DerefMut for HtmlPreviewSitesGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .as_deref_mut()
            .expect("HTML sites guard is active")
    }
}

#[cfg(test)]
impl Drop for HtmlPreviewSitesGuard<'_> {
    fn drop(&mut self) {
        self.inner.take();
        crate::path_auth::lock_order_test_probe::html_sites_released();
    }
}

impl HtmlPreviewServerState {
    fn lock_sites(&self) -> Result<HtmlPreviewSitesGuard<'_>, HtmlPreviewSitesRecoveryError> {
        let guard = match self.sites.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                #[cfg(test)]
                let mut sites = HtmlPreviewSitesGuard::new(guard);
                #[cfg(not(test))]
                let mut sites = guard;
                let drained_leases = sites.drain().map(|(_, site)| site.lease.clone()).collect();
                drop(sites);
                self.sites.clear_poison();
                return Err(HtmlPreviewSitesRecoveryError { drained_leases });
            }
        };
        #[cfg(test)]
        {
            Ok(HtmlPreviewSitesGuard::new(guard))
        }
        #[cfg(not(test))]
        {
            Ok(guard)
        }
    }

    pub(crate) fn invalidate_preview_leases(
        &self,
        leases: &HashSet<PreviewLeaseId>,
    ) -> Result<(), HtmlPreviewSitesRecoveryError> {
        if leases.is_empty() {
            return Ok(());
        }
        self.lock_sites()?
            .retain(|_, site| !leases.contains(&site.lease));
        Ok(())
    }

    #[cfg(test)]
    fn remove_committed_generation(
        &self,
        document: &Path,
        active_lease: &PreviewLeaseId,
    ) -> Result<(), String> {
        let mut sites = self
            .lock_sites()
            .map_err(HtmlPreviewSitesRecoveryError::into_message)?;
        if sites
            .get(document)
            .is_some_and(|site| &site.lease == active_lease)
        {
            sites.remove(document);
        }
        Ok(())
    }

    pub(crate) fn stop_all_sites(&self) -> Result<(), String> {
        self.lock_sites()
            .map_err(HtmlPreviewSitesRecoveryError::into_message)?
            .clear();
        Ok(())
    }

    fn start_site(
        &self,
        scope: AuthorizedPreviewScope,
        initial_content: String,
    ) -> Result<HtmlPreviewSite, String> {
        #[cfg(test)]
        if let Some(error) = self
            .next_site_start_error
            .lock()
            .map_err(|_| "HTML preview site-start test seam is poisoned".to_string())?
            .take()
        {
            return Err(error);
        }

        HtmlPreviewSite::start(scope, initial_content)
    }

    #[cfg(test)]
    fn fail_next_site_start(&self, error: impl Into<String>) -> Result<(), String> {
        *self
            .next_site_start_error
            .lock()
            .map_err(|_| "HTML preview site-start test seam is poisoned".to_string())? =
            Some(error.into());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn site_documents(&self) -> Result<HashSet<PathBuf>, String> {
        Ok(self
            .lock_sites()
            .map_err(HtmlPreviewSitesRecoveryError::into_message)?
            .keys()
            .cloned()
            .collect())
    }

    #[cfg(test)]
    fn site_lease_snapshot(&self) -> Result<HashSet<PreviewLeaseId>, String> {
        Ok(self
            .lock_sites()
            .map_err(HtmlPreviewSitesRecoveryError::into_message)?
            .values()
            .map(|site| site.lease.clone())
            .collect())
    }
}

fn header(name: &[u8], value: impl AsRef<[u8]>) -> Header {
    Header::from_bytes(name, value.as_ref()).expect("static preview response header is valid")
}

fn respond_bytes(request: Request, status: u16, mime_type: &str, body: Vec<u8>) {
    let length = body.len();
    let response = Response::new(
        StatusCode(status),
        vec![
            header(b"Content-Type", mime_type),
            header(b"Cache-Control", b"no-store"),
            header(b"X-Content-Type-Options", b"nosniff"),
        ],
        std::io::Cursor::new(body),
        Some(length),
        None,
    );
    let _ = request.respond(response);
}

fn requested_byte_range(request: &Request, length: usize) -> Result<Option<(usize, usize)>, ()> {
    let Some(value) = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Range"))
        .map(|header| header.value.as_str())
    else {
        return Ok(None);
    };
    let Some(range) = value.strip_prefix("bytes=") else {
        return Err(());
    };
    if length == 0 || range.contains(',') {
        return Err(());
    }
    let (start, end) = range.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<usize>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = length.saturating_sub(suffix);
        return Ok(Some((start, length - 1)));
    }
    let start = start.parse::<usize>().map_err(|_| ())?;
    if start >= length {
        return Err(());
    }
    let end = if end.is_empty() {
        length - 1
    } else {
        end.parse::<usize>().map_err(|_| ())?.min(length - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn respond_file(request: Request, mime_type: &str, mut file: File, length: usize) {
    match requested_byte_range(&request, length) {
        Ok(Some((start, end))) => {
            if file.seek(SeekFrom::Start(start as u64)).is_err() {
                respond_bytes(
                    request,
                    404,
                    "text/plain; charset=utf-8",
                    b"Not found".to_vec(),
                );
                return;
            }
            let range_length = end - start + 1;
            let response = Response::new(
                StatusCode(206),
                vec![
                    header(b"Content-Type", mime_type),
                    header(b"Content-Range", format!("bytes {start}-{end}/{length}")),
                    header(b"Accept-Ranges", b"bytes"),
                    header(b"Cache-Control", b"no-store"),
                    header(b"X-Content-Type-Options", b"nosniff"),
                ],
                file.take(range_length as u64),
                Some(range_length),
                None,
            );
            let _ = request.respond(response);
        }
        Ok(None) => {
            let response = Response::new(
                StatusCode(200),
                vec![
                    header(b"Content-Type", mime_type),
                    header(b"Accept-Ranges", b"bytes"),
                    header(b"Cache-Control", b"no-store"),
                    header(b"X-Content-Type-Options", b"nosniff"),
                ],
                file,
                Some(length),
                None,
            );
            let _ = request.respond(response);
        }
        Err(()) => {
            let response = Response::new(
                StatusCode(416),
                vec![
                    header(b"Content-Type", b"text/plain; charset=utf-8"),
                    header(b"Content-Range", format!("bytes */{length}")),
                    header(b"Cache-Control", b"no-store"),
                    header(b"X-Content-Type-Options", b"nosniff"),
                ],
                std::io::Cursor::new(Vec::new()),
                Some(0),
                None,
            );
            let _ = request.respond(response);
        }
    }
}

fn request_is_local(request: &Request, authority: &str) -> bool {
    let local_client = request
        .remote_addr()
        .is_some_and(|address| address.ip().is_loopback());
    let valid_host = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Host"))
        .is_some_and(|header| header.value.as_str() == authority);
    local_client && valid_host
}

fn route_request(
    request: Request,
    authority: &str,
    root: &Path,
    document: &Path,
    content: &Mutex<String>,
) {
    if !request_is_local(&request, authority) {
        respond_bytes(
            request,
            421,
            "text/plain; charset=utf-8",
            b"Misdirected request".to_vec(),
        );
        return;
    }
    if !matches!(request.method(), Method::Get | Method::Head) {
        let response = Response::new(
            StatusCode(405),
            vec![
                header(b"Content-Type", b"text/plain; charset=utf-8"),
                header(b"Allow", b"GET, HEAD"),
                header(b"Cache-Control", b"no-store"),
                header(b"X-Content-Type-Options", b"nosniff"),
            ],
            std::io::Cursor::new(b"Method not allowed".to_vec()),
            Some(18),
            None,
        );
        let _ = request.respond(response);
        return;
    }

    let encoded_path = request.url().split('?').next().unwrap_or_default();
    let Ok(decoded_path) = percent_decode_str(encoded_path).decode_utf8() else {
        respond_bytes(
            request,
            400,
            "text/plain; charset=utf-8",
            b"Invalid path".to_vec(),
        );
        return;
    };
    let relative_path = decoded_path.trim_start_matches('/');
    if relative_path.is_empty() {
        respond_bytes(
            request,
            404,
            "text/plain; charset=utf-8",
            b"Not found".to_vec(),
        );
        return;
    }
    let relative = Path::new(relative_path);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        respond_bytes(
            request,
            403,
            "text/plain; charset=utf-8",
            b"Forbidden".to_vec(),
        );
        return;
    }
    let requested = root.join(relative);
    let canonical = match normalize_existing_path(&requested) {
        Ok(path) if path_is_under(&path, root) && path.is_file() => path,
        _ => {
            respond_bytes(
                request,
                404,
                "text/plain; charset=utf-8",
                b"Not found".to_vec(),
            );
            return;
        }
    };

    if canonical == document {
        let live_content = match content.lock() {
            Ok(content) => content.clone(),
            Err(_) => {
                respond_bytes(
                    request,
                    500,
                    "text/plain; charset=utf-8",
                    b"Preview unavailable".to_vec(),
                );
                return;
            }
        };
        respond_bytes(
            request,
            200,
            "text/html; charset=utf-8",
            live_content.into_bytes(),
        );
        return;
    }

    match File::open(&canonical).and_then(|file| file.metadata().map(|metadata| (file, metadata))) {
        Ok((file, metadata)) => {
            let mime = mime_guess::from_path(&canonical).first_or_octet_stream();
            respond_file(request, mime.as_ref(), file, metadata.len() as usize);
        }
        Err(_) => respond_bytes(
            request,
            404,
            "text/plain; charset=utf-8",
            b"Not found".to_vec(),
        ),
    }
}

fn encoded_relative_path(root: &Path, document: &Path) -> Result<String, String> {
    let relative = document
        .strip_prefix(root)
        .map_err(|_| "HTML preview file escaped its authorized root".to_string())?;
    relative
        .components()
        .map(|component| match component {
            Component::Normal(segment) => segment
                .to_str()
                .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
                .ok_or_else(|| "HTML preview path is not valid UTF-8".to_string()),
            _ => Err("HTML preview path is invalid".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|segments| segments.join("/"))
}

impl HtmlPreviewSite {
    fn start(scope: AuthorizedPreviewScope, initial_content: String) -> Result<Self, String> {
        let (document, root, lease) = scope.into_parts();
        Self::start_parts(document, root, lease, initial_content)
    }

    fn start_parts(
        document: PathBuf,
        root: PathBuf,
        lease: PreviewLeaseId,
        initial_content: String,
    ) -> Result<Self, String> {
        let encoded_path = encoded_relative_path(&root, &document)?;
        #[cfg(test)]
        site_start_test_probe::loopback_bind_attempted();
        let server = Arc::new(
            Server::http("127.0.0.1:0")
                .map_err(|err| format!("Failed to start HTML preview server: {err}"))?,
        );
        let address = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "HTML preview server did not bind to an IP address".to_string())?;
        let authority = address.to_string();
        let content = Arc::new(Mutex::new(initial_content));
        let stop = Arc::new(AtomicBool::new(false));

        for worker_index in 0..PREVIEW_WORKERS {
            let worker_server = Arc::clone(&server);
            let worker_root = root.clone();
            let worker_document = document.clone();
            let worker_content = Arc::clone(&content);
            let worker_stop = Arc::clone(&stop);
            let worker_authority = authority.clone();
            let worker_builder =
                thread::Builder::new().name(format!("mmd-html-preview-{worker_index}"));
            #[cfg(test)]
            site_start_test_probe::worker_spawn_attempted();
            let spawn_result = worker_builder.spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    match worker_server.recv_timeout(Duration::from_millis(250)) {
                        Ok(Some(request)) => route_request(
                            request,
                            &worker_authority,
                            &worker_root,
                            &worker_document,
                            &worker_content,
                        ),
                        Ok(None) => {}
                        Err(_) if worker_stop.load(Ordering::Acquire) => break,
                        Err(_) => {}
                    }
                }
            });
            if let Err(err) = spawn_result {
                stop.store(true, Ordering::Release);
                for _ in 0..PREVIEW_WORKERS {
                    server.unblock();
                }
                return Err(format!("Failed to run HTML preview server: {err}"));
            }
        }

        Ok(Self {
            url: format!("http://{address}/{encoded_path}"),
            root,
            content,
            server,
            stop,
            lease,
        })
    }

    fn update(
        &mut self,
        content: &str,
        lease: PreviewLeaseId,
    ) -> Result<(String, PreviewLeaseId), String> {
        *self
            .content
            .lock()
            .map_err(|_| "HTML preview state is poisoned".to_string())? = content.to_string();
        let retired_lease = std::mem::replace(&mut self.lease, lease);
        Ok((self.url.clone(), retired_lease))
    }
}

impl Drop for HtmlPreviewSite {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for _ in 0..PREVIEW_WORKERS {
            self.server.unblock();
        }
    }
}

pub(crate) fn prepare_html_preview_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: &str,
) -> Result<String, String> {
    let path = path.as_ref().to_path_buf();
    let scope = preview_scope_for_file_inner(state, path)?;
    let reserved_lease = scope.lease().clone();
    let active_lease = reserved_lease.clone();
    let document = scope.document().to_path_buf();
    let site_transaction: Result<HtmlPreviewCommit, HtmlPreviewSiteTransactionError> = (|| {
        if WorkspaceFileKind::classify(scope.document()) != Some(WorkspaceFileKind::Html) {
            return Err(HtmlPreviewSiteTransactionError::Operation(
                "HTML preview requires an HTML file".into(),
            ));
        }
        let mut sites = state
            .html_preview_server
            .lock_sites()
            .map_err(HtmlPreviewSiteTransactionError::SitesRecovery)?;
        let root_changed = sites
            .get(&document)
            .is_some_and(|site| site.root.as_path() != scope.root());
        let (url, retired_lease) = if root_changed {
            let replacement = state
                .html_preview_server
                .start_site(scope, content.to_string())
                .map_err(HtmlPreviewSiteTransactionError::Operation)?;
            let url = replacement.url.clone();
            let replaced = sites
                .insert(document.clone(), replacement)
                .expect("root comparison found an existing preview site");
            let retired_lease = replaced.lease.clone();
            drop(replaced);
            (url, Some(retired_lease))
        } else if let Some(site) = sites.get_mut(&document) {
            let (_, _, lease) = scope.into_parts();
            let (url, retired_lease) = site
                .update(content, lease)
                .map_err(HtmlPreviewSiteTransactionError::Operation)?;
            (url, Some(retired_lease))
        } else {
            let site = state
                .html_preview_server
                .start_site(scope, content.to_string())
                .map_err(HtmlPreviewSiteTransactionError::Operation)?;
            let retired_lease = if sites.len() >= MAX_PREVIEW_SITES {
                if let Some(oldest) = sites.keys().next().cloned() {
                    sites.remove(&oldest).map(|site| site.lease.clone())
                } else {
                    None
                }
            } else {
                None
            };
            let url = site.url.clone();
            sites.insert(document.clone(), site);
            (url, retired_lease)
        };
        Ok(HtmlPreviewCommit {
            document,
            url,
            active_lease,
            retired_leases: retired_lease.into_iter().collect(),
        })
    })();
    let commit = match site_transaction {
        Ok(committed) => committed,
        Err(HtmlPreviewSiteTransactionError::Operation(error)) => {
            match retire_preview_lease_inner(state, &reserved_lease) {
                Ok(()) => return Err(error),
                Err(PreviewRetirementError::AuthorizationUnavailable(error)) => {
                    let _ = state.html_preview_server.stop_all_sites();
                    return Err(error);
                }
                #[cfg(test)]
                Err(PreviewRetirementError::Recoverable(error)) => return Err(error),
            }
        }
        Err(HtmlPreviewSiteTransactionError::SitesRecovery(recovery)) => {
            let (error, mut rollback_leases) = recovery.into_parts();
            rollback_leases.insert(reserved_lease.clone());
            match retire_preview_leases_inner(state, &rollback_leases) {
                Ok(()) => return Err(error),
                Err(PreviewRetirementError::AuthorizationUnavailable(error)) => {
                    let _ = state.html_preview_server.stop_all_sites();
                    return Err(error);
                }
                #[cfg(test)]
                Err(PreviewRetirementError::Recoverable(error)) => return Err(error),
            }
        }
    };
    let HtmlPreviewCommit {
        document: _document,
        url,
        active_lease: _active_lease,
        retired_leases,
    } = commit;
    match retire_preview_leases_inner(state, &retired_leases) {
        Ok(()) => Ok(url),
        Err(PreviewRetirementError::AuthorizationUnavailable(error)) => {
            let _ = state.html_preview_server.stop_all_sites();
            Err(error)
        }
        #[cfg(test)]
        Err(PreviewRetirementError::Recoverable(error)) => {
            state
                .html_preview_server
                .remove_committed_generation(&_document, &_active_lease)?;
            let mut rollback_leases = retired_leases;
            rollback_leases.insert(_active_lease);
            retire_preview_leases_inner(state, &rollback_leases)
                .map_err(|error| error.into_message())?;
            Err(error)
        }
    }
}

#[tauri::command]
pub(crate) fn prepare_html_preview(
    path: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    prepare_html_preview_inner(&state, path, &content)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        fs,
        io::{Read, Write},
        net::TcpStream,
    };

    use tempfile::tempdir;

    use crate::{
        commands::{open_directory_inner, rename_workspace_entry_inner},
        models::{MutationKind, MutationOutcome},
        path_auth::{
            authorize_directory_root_inner, authorize_file_inner,
            ensure_authorized_existing_file_inner, is_authorized_image_path,
            normalize_existing_path, relocate_authorized_path_prefix_inner,
            revoke_authorized_file_inner, revoke_authorized_path_prefix_inner,
        },
        state::AppState,
        workspace_file_kind::WorkspaceFileKind,
    };

    use super::{prepare_html_preview_inner, MAX_PREVIEW_SITES};

    fn http_request(url: &str, method: &str, headers: &[(&str, &str)]) -> Vec<u8> {
        let address_and_path = url.strip_prefix("http://").unwrap();
        let (address, path) = address_and_path.split_once('/').unwrap();
        http_request_with_host(address, path, method, address, headers)
    }

    fn http_request_with_host(
        address: &str,
        path: &str,
        method: &str,
        host: &str,
        headers: &[(&str, &str)],
    ) -> Vec<u8> {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "{method} /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n"
        )
        .unwrap();
        for (name, value) in headers {
            write!(stream, "{name}: {value}\r\n").unwrap();
        }
        write!(stream, "\r\n").unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    }

    fn http_get(url: &str) -> String {
        String::from_utf8(http_request(url, "GET", &[])).unwrap()
    }

    fn response_body(response: &[u8]) -> &[u8] {
        let separator = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        &response[separator + 4..]
    }

    #[test]
    fn serves_live_html_and_relative_assets_over_loopback_http() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<h1>Saved</h1>").unwrap();
        fs::write(site.join("app.js"), "window.previewLoaded = true;").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let url = prepare_html_preview_inner(
            &state,
            &html,
            "<h1>Unsaved</h1><script src=\"app.js\"></script>",
        )
        .unwrap();

        assert!(url.starts_with("http://127.0.0.1:"));
        let html_response = http_get(&url);
        assert!(html_response.starts_with("HTTP/1.1 200"));
        assert!(html_response.contains("<h1>Unsaved</h1>"));
        assert!(!html_response
            .to_ascii_lowercase()
            .contains("access-control-allow-origin"));

        let asset_url = format!("{}/app.js", url.rsplit_once('/').unwrap().0);
        let asset_response = http_get(&asset_url);
        assert!(asset_response.starts_with("HTTP/1.1 200"));
        assert!(asset_response.contains("window.previewLoaded = true;"));

        let updated_url =
            prepare_html_preview_inner(&state, &html, "<h1>Updated again</h1>").unwrap();
        assert_eq!(updated_url, url);
        assert!(http_get(&updated_url).contains("<h1>Updated again</h1>"));
    }

    #[test]
    fn isolates_preview_documents_on_separate_origins() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.html");
        let second = dir.path().join("second.html");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let first_url = prepare_html_preview_inner(&state, &first, "first").unwrap();
        let second_url = prepare_html_preview_inner(&state, &second, "second").unwrap();
        let first_origin = first_url.rsplit_once('/').unwrap().0;
        let second_origin = second_url.rsplit_once('/').unwrap().0;

        assert_ne!(first_origin, second_origin);
    }

    #[test]
    fn gives_workspace_pages_normal_root_relative_url_semantics() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<script src=\"/root.js\"></script>").unwrap();
        fs::write(dir.path().join("root.js"), "window.fromRoot = true;").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let url = prepare_html_preview_inner(&state, &html, "<script src=\"/root.js\"></script>")
            .unwrap();
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();

        let response = http_get(&format!("http://{address}/root.js"));
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("window.fromRoot = true;"));
    }

    #[test]
    fn preview_scope_uses_the_most_specific_authorized_root() {
        let workspace = tempdir().unwrap();
        let site = workspace.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<script src=\"app.js\"></script>").unwrap();
        fs::write(site.join("app.js"), "window.fromSite = true;").unwrap();
        fs::write(workspace.path().join("root.js"), "window.fromRoot = true;").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();
        authorize_file_inner(&state, html.clone()).unwrap();

        let url =
            prepare_html_preview_inner(&state, &html, "<script src=\"app.js\"></script>").unwrap();
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();

        assert!(!url.contains("/site/"));
        assert!(http_get(&format!("http://{address}/app.js")).starts_with("HTTP/1.1 200"));
        assert!(http_get(&format!("http://{address}/root.js")).starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn reprepare_applies_changed_authorized_scope_root() {
        let workspace = tempdir().unwrap();
        let site = workspace.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<script src=\"/scope.js\"></script>").unwrap();
        fs::write(
            workspace.path().join("scope.js"),
            "window.previewScope = 'wide';",
        )
        .unwrap();
        fs::write(site.join("scope.js"), "window.previewScope = 'narrow';").unwrap();
        let canonical_document = normalize_existing_path(&html).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        let wide_url =
            prepare_html_preview_inner(&state, &html, "<script src=\"/scope.js\"></script>")
                .unwrap();
        let wide_address = wide_url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let wide_asset_url = format!("http://{wide_address}/scope.js");
        assert!(http_get(&wide_asset_url).contains("previewScope = 'wide'"));
        let old_worker_stop = state
            .html_preview_server
            .sites
            .lock()
            .unwrap()
            .get(&canonical_document)
            .unwrap()
            .stop
            .clone();

        authorize_file_inner(&state, html.clone()).unwrap();
        let (narrow_url, reprepare_events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &html, "<script src=\"/scope.js\"></script>")
        });
        let narrow_url = narrow_url.unwrap();

        assert_ne!(
            narrow_url, wide_url,
            "changed authorized root reused the old preview worker"
        );
        let narrow_address = narrow_url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let narrow_asset_url = format!("http://{narrow_address}/scope.js");
        assert!(http_get(&narrow_asset_url).contains("previewScope = 'narrow'"));
        assert!(old_worker_stop.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            reprepare_events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn failed_site_commit_rolls_back_only_new_preview_lease() {
        let workspace = tempdir().unwrap();
        let site = workspace.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        fs::write(&html, "saved").unwrap();
        fs::write(
            workspace.path().join("scope.js"),
            "window.previewScope = 'wide';",
        )
        .unwrap();
        fs::write(site.join("scope.js"), "window.previewScope = 'narrow';").unwrap();
        let canonical_document = normalize_existing_path(&html).unwrap();
        let canonical_wide_root = normalize_existing_path(workspace.path()).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        let old_content = "<h1>old preview</h1><script src=\"/scope.js\"></script>";
        let old_url = prepare_html_preview_inner(&state, &html, old_content).unwrap();
        let old_address = old_url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let old_asset_url = format!("http://{old_address}/scope.js");
        let (old_lease, old_stop, old_server, old_content_state) = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let old_site = sites.get(&canonical_document).unwrap();
            (
                old_site.lease.clone(),
                old_site.stop.clone(),
                old_site.server.clone(),
                old_site.content.clone(),
            )
        };

        authorize_file_inner(&state, html.clone()).unwrap();
        let leases_before = state.file_authorization().preview_lease_snapshot().unwrap();
        let sites_before = state.html_preview_server.site_documents().unwrap();
        assert_eq!(leases_before, HashSet::from([old_lease.clone()]));
        state
            .html_preview_server
            .fail_next_site_start("injected replacement start failure")
            .unwrap();

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &html, "<h1>replacement preview</h1>")
        });
        let error = result.expect_err("injected replacement start must fail");
        assert_eq!(error, "injected replacement start failure");
        let leases_after = state.file_authorization().preview_lease_snapshot().unwrap();

        assert_eq!(
            leases_after, leases_before,
            "failed site commit leaked the newly reserved preview generation"
        );
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            sites_before,
            "failed replacement changed the preview site map"
        );
        {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let retained = sites.get(&canonical_document).unwrap();
            assert_eq!(retained.url, old_url);
            assert_eq!(retained.root, canonical_wide_root);
            assert_eq!(retained.lease, old_lease);
            assert!(std::sync::Arc::ptr_eq(&retained.stop, &old_stop));
            assert!(std::sync::Arc::ptr_eq(&retained.server, &old_server));
            assert!(std::sync::Arc::ptr_eq(
                &retained.content,
                &old_content_state
            ));
        }
        assert!(!old_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(http_get(&old_url).contains("<h1>old preview</h1>"));
        assert!(http_get(&old_asset_url).contains("previewScope = 'wide'"));
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn authorization_unavailable_during_failed_commit_rollback_stops_all_preview_sites() {
        let workspace = tempdir().unwrap();
        let target_root = workspace.path().join("target");
        fs::create_dir(&target_root).unwrap();
        let target = target_root.join("index.html");
        let unrelated = workspace.path().join("unrelated.html");
        fs::write(&target, "saved target").unwrap();
        fs::write(&unrelated, "saved unrelated").unwrap();
        let canonical_target = normalize_existing_path(&target).unwrap();
        let canonical_unrelated = normalize_existing_path(&unrelated).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        let target_url = prepare_html_preview_inner(&state, &target, "live target").unwrap();
        let unrelated_url =
            prepare_html_preview_inner(&state, &unrelated, "live unrelated").unwrap();
        assert!(http_get(&target_url).contains("live target"));
        assert!(http_get(&unrelated_url).contains("live unrelated"));
        let (target_stop, unrelated_stop) = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            (
                sites.get(&canonical_target).unwrap().stop.clone(),
                sites.get(&canonical_unrelated).unwrap().stop.clone(),
            )
        };

        authorize_file_inner(&state, target.clone()).unwrap();
        state
            .html_preview_server
            .fail_next_site_start("injected replacement start failure")
            .unwrap();
        state
            .file_authorization()
            .fail_next_preview_retirement_as_unavailable(
                "injected authorization unavailable during failed-commit rollback",
            )
            .unwrap();

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &target, "uncommitted replacement")
        });
        let error = result.expect_err("authorization-unavailable rollback must not return a URL");
        assert_eq!(
            error,
            "injected authorization unavailable during failed-commit rollback"
        );
        let documents_after = state.html_preview_server.site_documents().unwrap();

        assert!(
            documents_after.is_empty(),
            "authorization-unavailable rollback preserved unverified preview sites: {documents_after:?}"
        );
        assert!(target_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(unrelated_stop.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
            ]
        );
    }

    #[test]
    fn capacity_eviction_revokes_exact_evicted_preview_lease() {
        let workspace = tempdir().unwrap();
        let documents = (0..=MAX_PREVIEW_SITES)
            .map(|index| {
                let document = workspace.path().join(format!("site-{index}.html"));
                fs::write(&document, format!("saved site {index}")).unwrap();
                document
            })
            .collect::<Vec<_>>();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        for (index, document) in documents.iter().take(MAX_PREVIEW_SITES).enumerate() {
            prepare_html_preview_inner(&state, document, &format!("live site {index}")).unwrap();
        }

        let authorization_before = state.file_authorization().preview_lease_snapshot().unwrap();
        let sites_before = state.html_preview_server.site_lease_snapshot().unwrap();
        assert_eq!(authorization_before, sites_before);
        assert_eq!(sites_before.len(), MAX_PREVIEW_SITES);
        let worker_stops_before = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            sites
                .values()
                .map(|site| (site.lease.clone(), site.stop.clone()))
                .collect::<Vec<_>>()
        };

        let new_document = normalize_existing_path(&documents[MAX_PREVIEW_SITES]).unwrap();
        let (new_url, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(
                &state,
                &new_document,
                &format!("live site {MAX_PREVIEW_SITES}"),
            )
        });
        let new_url = new_url.unwrap();
        let sites_after = state.html_preview_server.site_lease_snapshot().unwrap();
        let authorization_after = state.file_authorization().preview_lease_snapshot().unwrap();
        let evicted = sites_before
            .difference(&sites_after)
            .cloned()
            .collect::<Vec<_>>();
        let added = sites_after
            .difference(&sites_before)
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(evicted.len(), 1, "capacity must evict exactly one site");
        assert_eq!(added.len(), 1, "capacity must add exactly one site");
        let evicted_lease = &evicted[0];
        let new_lease = &added[0];
        assert_eq!(
            authorization_after, sites_after,
            "capacity eviction retained authorization for the evicted preview lease"
        );
        assert!(!authorization_after.contains(evicted_lease));
        let evicted_stop = worker_stops_before
            .iter()
            .find_map(|(lease, stop)| (lease == evicted_lease).then_some(stop))
            .unwrap();
        assert!(evicted_stop.load(std::sync::atomic::Ordering::Acquire));
        {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let new_site = sites.get(&new_document).unwrap();
            assert_eq!(&new_site.lease, new_lease);
            assert_eq!(new_site.url, new_url);
            assert!(!new_site.stop.load(std::sync::atomic::Ordering::Acquire));
        }
        assert!(http_get(&new_url).contains(&format!("live site {MAX_PREVIEW_SITES}")));
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn post_commit_retirement_failure_removes_committed_generation_and_preserves_unrelated_site() {
        let workspace = tempdir().unwrap();
        let target = workspace.path().join("target.html");
        let unrelated = workspace.path().join("unrelated.html");
        fs::write(&target, "saved target").unwrap();
        fs::write(&unrelated, "saved unrelated").unwrap();
        let canonical_target = normalize_existing_path(&target).unwrap();
        let canonical_unrelated = normalize_existing_path(&unrelated).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        let target_url = prepare_html_preview_inner(&state, &target, "old target content").unwrap();
        let unrelated_url =
            prepare_html_preview_inner(&state, &unrelated, "stable unrelated content").unwrap();
        let (
            old_target_lease,
            target_stop,
            unrelated_lease,
            unrelated_stop,
            unrelated_server,
            unrelated_content,
        ) = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let target_site = sites.get(&canonical_target).unwrap();
            let unrelated_site = sites.get(&canonical_unrelated).unwrap();
            (
                target_site.lease.clone(),
                target_site.stop.clone(),
                unrelated_site.lease.clone(),
                unrelated_site.stop.clone(),
                unrelated_site.server.clone(),
                unrelated_site.content.clone(),
            )
        };
        let leases_before = state.file_authorization().preview_lease_snapshot().unwrap();
        assert_eq!(
            leases_before,
            HashSet::from([old_target_lease.clone(), unrelated_lease.clone()])
        );
        assert_eq!(
            state.html_preview_server.site_lease_snapshot().unwrap(),
            leases_before
        );
        state
            .file_authorization()
            .fail_next_preview_retirement("injected post-commit retirement failure")
            .unwrap();

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &target, "new committed target content")
        });
        let error = result.expect_err("injected post-commit retirement must fail");
        assert_eq!(error, "injected post-commit retirement failure");
        let documents_after = state.html_preview_server.site_documents().unwrap();
        let target_remained = documents_after.contains(&canonical_target);
        let serves_new_content =
            target_remained && http_get(&target_url).contains("new committed target content");

        assert!(
            !target_remained,
            "post-commit cleanup failure left the committed target live; serves_new_content={serves_new_content}"
        );
        assert!(target_stop.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            documents_after,
            HashSet::from([canonical_unrelated.clone()])
        );
        {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let retained = sites.get(&canonical_unrelated).unwrap();
            assert_eq!(retained.url, unrelated_url);
            assert_eq!(retained.lease, unrelated_lease);
            assert!(std::sync::Arc::ptr_eq(&retained.stop, &unrelated_stop));
            assert!(std::sync::Arc::ptr_eq(&retained.server, &unrelated_server));
            assert!(std::sync::Arc::ptr_eq(
                &retained.content,
                &unrelated_content
            ));
        }
        assert!(!unrelated_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(http_get(&unrelated_url).contains("stable unrelated content"));
        let expected_leases = HashSet::from([unrelated_lease]);
        assert_eq!(
            state.html_preview_server.site_lease_snapshot().unwrap(),
            expected_leases
        );
        assert_eq!(
            state.file_authorization().preview_lease_snapshot().unwrap(),
            expected_leases
        );
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn authorization_unavailable_during_cleanup_stops_all_preview_sites() {
        let workspace = tempdir().unwrap();
        let target = workspace.path().join("target.html");
        let unrelated = workspace.path().join("unrelated.html");
        fs::write(&target, "saved target").unwrap();
        fs::write(&unrelated, "saved unrelated").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        prepare_html_preview_inner(&state, &target, "old target content").unwrap();
        prepare_html_preview_inner(&state, &unrelated, "unrelated content").unwrap();
        let (target_stop, unrelated_stop) = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            (
                sites
                    .get(&normalize_existing_path(&target).unwrap())
                    .unwrap()
                    .stop
                    .clone(),
                sites
                    .get(&normalize_existing_path(&unrelated).unwrap())
                    .unwrap()
                    .stop
                    .clone(),
            )
        };
        state
            .file_authorization()
            .fail_next_preview_retirement_as_unavailable(
                "injected authorization unavailable during preview cleanup",
            )
            .unwrap();

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &target, "new committed target content")
        });
        let error = result.expect_err("authorization-unavailable cleanup must not return a URL");
        assert_eq!(
            error,
            "injected authorization unavailable during preview cleanup"
        );
        let documents_after = state.html_preview_server.site_documents().unwrap();

        assert!(
            documents_after.is_empty(),
            "authorization-unavailable cleanup preserved preview sites: {documents_after:?}"
        );
        assert!(target_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(unrelated_stop.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
            ]
        );
    }

    #[test]
    fn relocate_prefix_relocates_internal_grants_and_invalidates_old_preview_sites() {
        let container = tempdir().unwrap();
        let old_root = container.path().join("old");
        fs::create_dir(&old_root).unwrap();
        let old_document = old_root.join("index.html");
        let old_asset = old_root.join("asset.png");
        fs::write(&old_document, "<img src=\"asset.png\">").unwrap();
        fs::write(&old_asset, b"png").unwrap();
        let canonical_old_root = normalize_existing_path(&old_root).unwrap();
        let canonical_old_document = normalize_existing_path(&old_document).unwrap();
        let state = AppState::default();
        authorize_file_inner(&state, old_document.clone()).unwrap();
        let old_url = prepare_html_preview_inner(&state, &old_document, "old").unwrap();

        let new_root = container.path().join("new");
        fs::rename(&old_root, &new_root).unwrap();
        let canonical_new_root = normalize_existing_path(&new_root).unwrap();
        let new_document = canonical_new_root.join("index.html");
        let new_asset = normalize_existing_path(canonical_new_root.join("asset.png")).unwrap();
        relocate_authorized_path_prefix_inner(&state, &canonical_old_root, &canonical_new_root)
            .unwrap();

        assert!(!state
            .html_preview_server
            .sites
            .lock()
            .unwrap()
            .contains_key(&canonical_old_document));
        assert!(ensure_authorized_existing_file_inner(&state, &new_document).is_ok());
        assert!(is_authorized_image_path(&state, &new_asset).unwrap());

        let new_url = prepare_html_preview_inner(&state, &new_document, "new").unwrap();
        assert_ne!(new_url, old_url);
    }

    #[test]
    fn poisoned_site_map_relocation_retires_unrelated_drained_preview_leases() {
        let workspace = tempdir().unwrap();
        let old_root = workspace.path().join("old");
        fs::create_dir(&old_root).unwrap();
        let target_document = old_root.join("target.html");
        let unrelated_document = workspace.path().join("unrelated.html");
        fs::write(&target_document, "target").unwrap();
        fs::write(&unrelated_document, "unrelated").unwrap();
        let canonical_old_root = normalize_existing_path(&old_root).unwrap();
        let canonical_target_document = normalize_existing_path(&target_document).unwrap();
        let canonical_unrelated_document = normalize_existing_path(&unrelated_document).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();
        prepare_html_preview_inner(&state, &target_document, "live target").unwrap();
        prepare_html_preview_inner(&state, &unrelated_document, "live unrelated").unwrap();

        let (target_lease, target_stop, unrelated_lease, unrelated_stop) = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let target = sites.get(&canonical_target_document).unwrap();
            let unrelated = sites.get(&canonical_unrelated_document).unwrap();
            (
                target.lease.clone(),
                target.stop.clone(),
                unrelated.lease.clone(),
                unrelated.stop.clone(),
            )
        };
        assert_eq!(
            state.file_authorization().preview_lease_snapshot().unwrap(),
            HashSet::from([target_lease, unrelated_lease])
        );

        let new_root = workspace.path().join("new");
        fs::rename(&old_root, &new_root).unwrap();
        let canonical_new_root = normalize_existing_path(&new_root).unwrap();
        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sites = state.html_preview_server.sites.lock().unwrap();
            panic!("injected HTML site map poison before relocation");
        }));
        assert!(poison.is_err());
        assert!(state.html_preview_server.sites.is_poisoned());

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            relocate_authorized_path_prefix_inner(&state, &canonical_old_root, &canonical_new_root)
        });
        let error = result.expect_err("poisoned preview relocation must fail closed");

        assert_eq!(
            error,
            "HTML preview server state was poisoned; all preview sites were stopped"
        );
        assert!(!state.html_preview_server.sites.is_poisoned());
        assert!(state.html_preview_server.sites.lock().unwrap().is_empty());
        assert!(target_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(unrelated_stop.load(std::sync::atomic::Ordering::Acquire));
        let preview_leases = state.file_authorization().preview_lease_snapshot().unwrap();
        assert!(
            preview_leases.is_empty(),
            "poisoned relocation left drained preview leases authorized: {preview_leases:?}; events={events:?}"
        );
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn poisoned_preview_cleanup_after_rename_returns_indeterminate_outcome() {
        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.html");
        let target = workspace.path().join("renamed.html");
        fs::write(&source, "draft").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        prepare_html_preview_inner(&state, &source, "live draft").unwrap();
        let canonical_source = normalize_existing_path(&source).unwrap();

        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sites = state.html_preview_server.sites.lock().unwrap();
            panic!("injected HTML site map poison before rename cleanup");
        }));
        assert!(poison.is_err());

        let outcome = rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &canonical_source,
            "renamed.html",
        )
        .unwrap();

        let MutationOutcome::Indeterminate {
            operation: MutationKind::Rename,
            paths,
            recovery_message,
        } = outcome
        else {
            panic!("post-commit preview failure must be indeterminate");
        };
        assert_eq!(
            paths,
            [
                canonical_source.to_string_lossy().to_string(),
                target.canonicalize().unwrap().to_string_lossy().to_string(),
            ]
        );
        assert_eq!(
            recovery_message,
            "HTML preview server state was poisoned; all preview sites were stopped"
        );
        assert!(!canonical_source.exists());
        assert!(target.is_file());
        assert!(state
            .file_authorization()
            .preview_lease_snapshot()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn poisoned_site_map_file_revocation_retires_unrelated_drained_preview_leases() {
        let workspace = tempdir().unwrap();
        let target_document = workspace.path().join("target.html");
        let unrelated_document = workspace.path().join("unrelated.html");
        fs::write(&target_document, "target").unwrap();
        fs::write(&unrelated_document, "unrelated").unwrap();
        let canonical_target_document = normalize_existing_path(&target_document).unwrap();
        let canonical_unrelated_document = normalize_existing_path(&unrelated_document).unwrap();
        let state = AppState::default();
        let (authorized_target, ()) = state
            .file_authorization()
            .open_standalone_file(&target_document, |_| Ok(()), |_| Ok(()))
            .unwrap();
        authorize_file_inner(&state, unrelated_document.clone()).unwrap();
        prepare_html_preview_inner(&state, &target_document, "live target").unwrap();
        prepare_html_preview_inner(&state, &unrelated_document, "live unrelated").unwrap();

        let (target_lease, target_stop, unrelated_lease, unrelated_stop) = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            let target = sites.get(&canonical_target_document).unwrap();
            let unrelated = sites.get(&canonical_unrelated_document).unwrap();
            (
                target.lease.clone(),
                target.stop.clone(),
                unrelated.lease.clone(),
                unrelated.stop.clone(),
            )
        };
        assert_eq!(
            state.file_authorization().preview_lease_snapshot().unwrap(),
            HashSet::from([target_lease, unrelated_lease])
        );

        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sites = state.html_preview_server.sites.lock().unwrap();
            panic!("injected HTML site map poison before file revocation");
        }));
        assert!(poison.is_err());
        assert!(state.html_preview_server.sites.is_poisoned());

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            revoke_authorized_file_inner(&state, &authorized_target)
        });
        let error = result.expect_err("poisoned preview file revocation must fail closed");

        assert_eq!(
            error,
            "HTML preview server state was poisoned; all preview sites were stopped"
        );
        assert!(!state.html_preview_server.sites.is_poisoned());
        assert!(state.html_preview_server.sites.lock().unwrap().is_empty());
        assert!(target_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(unrelated_stop.load(std::sync::atomic::Ordering::Acquire));
        let preview_leases = state.file_authorization().preview_lease_snapshot().unwrap();
        assert!(
            preview_leases.is_empty(),
            "poisoned file revocation left drained preview leases authorized: {preview_leases:?}; events={events:?}"
        );
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn poisoned_site_map_revocation_stops_all_preview_workers() {
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
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();
        prepare_html_preview_inner(&state, &removed_document, "removed").unwrap();
        prepare_html_preview_inner(&state, &retained_document, "retained").unwrap();

        let worker_stops = {
            let sites = state.html_preview_server.sites.lock().unwrap();
            vec![
                sites.get(&canonical_removed_document).unwrap().stop.clone(),
                sites
                    .get(&canonical_retained_document)
                    .unwrap()
                    .stop
                    .clone(),
            ]
        };
        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sites = state.html_preview_server.sites.lock().unwrap();
            panic!("injected HTML site map poison");
        }));
        assert!(poison.is_err());
        assert!(state.html_preview_server.sites.is_poisoned());

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            revoke_authorized_path_prefix_inner(&state, &canonical_removed_root)
        });
        let error = result.expect_err("poisoned preview revocation must fail closed");

        let workers_stopped = worker_stops
            .iter()
            .all(|stop| stop.load(std::sync::atomic::Ordering::Acquire));
        let sites_empty = state
            .html_preview_server
            .sites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        assert!(
            workers_stopped && sites_empty,
            "poisoned site map did not fail closed: workers_stopped={workers_stopped}, sites_empty={sites_empty}"
        );
        assert!(!state.html_preview_server.sites.is_poisoned());
        assert_eq!(
            error,
            "HTML preview server state was poisoned; all preview sites were stopped"
        );
        let preview_leases = state.file_authorization().preview_lease_snapshot().unwrap();
        assert!(
            preview_leases.is_empty(),
            "poison recovery left preview leases authorized: {preview_leases:?}"
        );
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[test]
    fn poisoned_site_map_prepare_retires_drained_and_reserved_preview_leases() {
        let workspace = tempdir().unwrap();
        let old_document = workspace.path().join("old.html");
        let new_document = workspace.path().join("new.html");
        fs::write(&old_document, "saved old").unwrap();
        fs::write(&new_document, "saved new").unwrap();
        let canonical_old_document = normalize_existing_path(&old_document).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        let old_url = prepare_html_preview_inner(&state, &old_document, "live old").unwrap();
        assert!(http_get(&old_url).contains("live old"));
        let old_worker_stop = state
            .html_preview_server
            .sites
            .lock()
            .unwrap()
            .get(&canonical_old_document)
            .unwrap()
            .stop
            .clone();
        assert_eq!(
            state
                .file_authorization()
                .preview_lease_snapshot()
                .unwrap()
                .len(),
            1
        );
        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sites = state.html_preview_server.sites.lock().unwrap();
            panic!("injected HTML site map poison before prepare");
        }));
        assert!(poison.is_err());
        assert!(state.html_preview_server.sites.is_poisoned());

        let (result, events) = crate::path_auth::lock_order_test_probe::trace(|| {
            prepare_html_preview_inner(&state, &new_document, "uncommitted new")
        });
        let error = result.expect_err("prepare against poisoned preview state must fail closed");

        assert_eq!(
            error,
            "HTML preview server state was poisoned; all preview sites were stopped"
        );
        assert!(old_worker_stop.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.html_preview_server.sites.is_poisoned());
        assert!(state.html_preview_server.sites.lock().unwrap().is_empty());
        let preview_leases = state.file_authorization().preview_lease_snapshot().unwrap();
        assert!(
            preview_leases.is_empty(),
            "poisoned prepare left drained or reserved preview leases authorized: {preview_leases:?}"
        );
        assert_eq!(
            events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_html_path_is_rejected_before_loopback_resources_start() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let workspace = tempdir().unwrap();
        let valid_document = workspace.path().join("preview.html");
        fs::write(&valid_document, "saved HTML").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();
        let scope =
            crate::path_auth::preview_scope_for_file_inner(&state, &valid_document).unwrap();
        let (_, root, lease) = scope.into_parts();
        let document = root.join(OsString::from_vec(b"preview-\xff.html".to_vec()));
        assert_eq!(
            WorkspaceFileKind::classify(&document),
            Some(WorkspaceFileKind::Html)
        );

        let (result, events) = super::site_start_test_probe::trace(|| {
            super::HtmlPreviewSite::start_parts(
                document,
                root,
                lease.clone(),
                "uncommitted content".to_string(),
            )
        });
        let error = match result {
            Ok(_) => panic!("non-UTF-8 preview path must be rejected"),
            Err(error) => error,
        };
        crate::path_auth::retire_preview_lease_inner(&state, &lease)
            .map_err(crate::path_auth::PreviewRetirementError::into_message)
            .unwrap();

        assert_eq!(error, "HTML preview path is not valid UTF-8");
        assert!(
            events.is_empty(),
            "loopback resources started before path validation: {events:?}"
        );
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());
        assert!(state
            .file_authorization()
            .preview_lease_snapshot()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_unauthorized_html_files() {
        let dir = tempdir().unwrap();
        let html = dir.path().join("index.html");
        fs::write(&html, "<h1>Saved</h1>").unwrap();

        let error =
            prepare_html_preview_inner(&AppState::default(), &html, "<h1>Draft</h1>").unwrap_err();

        assert!(error.contains("outside the user-authorized"));
    }

    #[test]
    fn file_authorization_serves_sibling_assets_but_not_parent_traversal() {
        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        fs::write(&html, "<h1>Saved</h1>").unwrap();
        fs::write(site.join("app.js"), "window.ok = true;").unwrap();
        fs::write(dir.path().join("secret.txt"), "secret").unwrap();
        let state = AppState::default();
        authorize_file_inner(&state, html.clone()).unwrap();

        let url =
            prepare_html_preview_inner(&state, &html, "<script src=\"app.js\"></script>").unwrap();
        let base = url.rsplit_once('/').unwrap().0;

        assert!(http_get(&format!("{base}/app.js")).starts_with("HTTP/1.1 200"));
        assert!(http_get(&format!("{base}/%2e%2e/secret.txt")).starts_with("HTTP/1.1 403"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_assets_that_escape_the_preview_root() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let site = dir.path().join("site");
        fs::create_dir(&site).unwrap();
        let html = site.join("index.html");
        let secret = dir.path().join("secret.txt");
        fs::write(&html, "<h1>Saved</h1>").unwrap();
        fs::write(&secret, "secret").unwrap();
        symlink(&secret, site.join("secret-link.txt")).unwrap();
        let state = AppState::default();
        authorize_file_inner(&state, html.clone()).unwrap();

        let url = prepare_html_preview_inner(&state, &html, "<h1>Draft</h1>").unwrap();
        let linked_url = format!("{}/secret-link.txt", url.rsplit_once('/').unwrap().0);

        assert!(http_get(&linked_url).starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn rejects_unknown_paths_and_forged_hosts() {
        let dir = tempdir().unwrap();
        let html = dir.path().join("index.html");
        fs::write(&html, "<h1>Saved</h1>").unwrap();
        let state = AppState::default();
        authorize_file_inner(&state, html.clone()).unwrap();
        let url = prepare_html_preview_inner(&state, &html, "<h1>Draft</h1>").unwrap();
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();

        assert!(
            http_get(&format!("http://{address}/invalid/index.html")).starts_with("HTTP/1.1 404")
        );

        let response =
            http_request_with_host(address, "index.html", "GET", "attacker.example", &[]);
        assert!(String::from_utf8(response)
            .unwrap()
            .starts_with("HTTP/1.1 421"));
    }

    #[test]
    fn supports_head_and_byte_ranges_for_preview_assets() {
        let dir = tempdir().unwrap();
        let html = dir.path().join("index.html");
        fs::write(&html, "<video src=\"clip.mp4\"></video>").unwrap();
        fs::write(dir.path().join("clip.mp4"), b"0123456789").unwrap();
        let state = AppState::default();
        authorize_file_inner(&state, html.clone()).unwrap();
        let url =
            prepare_html_preview_inner(&state, &html, "<video src=\"clip.mp4\"></video>").unwrap();
        let asset_url = format!("{}/clip.mp4", url.rsplit_once('/').unwrap().0);

        let head = http_request(&asset_url, "HEAD", &[]);
        let head_text = String::from_utf8(head.clone()).unwrap();
        assert!(head_text.starts_with("HTTP/1.1 200"));
        assert!(head_text
            .to_ascii_lowercase()
            .contains("content-length: 10"));
        assert!(response_body(&head).is_empty());

        let range = http_request(&asset_url, "GET", &[("Range", "bytes=2-5")]);
        let headers = String::from_utf8_lossy(&range[..range.len() - response_body(&range).len()]);
        assert!(headers.starts_with("HTTP/1.1 206"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("content-range: bytes 2-5/10"));
        assert_eq!(response_body(&range), b"2345");
    }
}
