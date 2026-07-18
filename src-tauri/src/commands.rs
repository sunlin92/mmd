use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{
    ffi::{CStr, CString},
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::{OsStrExt, OsStringExt},
    },
    path::PathBuf,
};

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::{
    docx_preflight::{preflight_docx_zip, DOCX_SOURCE_LIMIT_BYTES},
    excalidraw_scene::{default_excalidraw_scene, validate_excalidraw_scene},
    image_resolver::resolve_relative_image_path_inner,
    markdown_files::read_markdown_file,
    models::{
        DeleteWorkspaceEntryResponse, MutationCommitReceipt, MutationOutcome, OpenCommitResult,
        OpenCommitStatus, OpenFileResponse, PreparedOpenFileResponse, RecentFilesSnapshot,
        RenameWorkspaceEntryResponse, SnapshotReceipt, WorkspaceMutation, WorkspaceSessionRestore,
        WorkspaceSnapshot,
    },
    native_menu,
    path_auth::{
        commit_indeterminate_delete_inner, delete_authorized_workspace_entry_inner,
        ensure_authorized_existing_file_inner, move_authorized_workspace_entry_inner,
        normalize_existing_path, normalize_file_for_write, path_is_under,
        rename_authorized_workspace_entry_inner,
        resolve_authorized_workspace_directory_for_token_inner,
        resolve_authorized_workspace_root_for_token_inner, save_document_as_inner,
        write_authorized_document_inner, AuthorizedDeleteOutcome, AuthorizedRenameOutcome,
        AuthorizedWorkspace, AuthorizedWriteOutcome, DeleteFileObservation, RenameErrorObservation,
        WorkspaceSnapshotSource,
    },
    state::AppState,
    workspace_file_kind::{ContentMode, WorkspaceFileKind},
    workspace_session::WorkspaceSessionRecord,
    workspace_snapshot::{capture_workspace_snapshot, CapturedWorkspaceSnapshot},
};

#[cfg(test)]
use crate::path_auth::authorize_workspace_file_inner;

#[cfg(test)]
use crate::path_auth::{
    authorize_directory_root_inner, ensure_authorized_directory_inner,
    ensure_authorized_write_file_inner, GrantStatus, WorkspaceCandidate,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum ObservedPathKind {
    File,
    Directory,
    Symlink,
}

const APP_FEEDBACK_ERROR_EVENT: &str = "mmd:app-feedback-error";
const RECENT_MENU_SYNC_ERROR: &str = "Recent files menu synchronization failed";
const IMAGE_SOURCE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const PDF_SOURCE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

fn refresh_recent_menu_with_retry(
    snapshot: &RecentFilesSnapshot,
    reload: impl FnOnce() -> Result<RecentFilesSnapshot, String>,
    mut refresh: impl FnMut(&RecentFilesSnapshot) -> Result<(), String>,
) -> Result<(), String> {
    if refresh(snapshot).is_ok() {
        return Ok(());
    }
    let reloaded = reload().map_err(|_| RECENT_MENU_SYNC_ERROR.to_string())?;
    refresh(&reloaded).map_err(|_| RECENT_MENU_SYNC_ERROR.to_string())
}

fn emit_app_feedback_error(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit_to("main", APP_FEEDBACK_ERROR_EVENT, message.into());
}

fn converge_recent_menu(app: &AppHandle, state: &AppState, snapshot: &RecentFilesSnapshot) {
    state.set_native_recent_files(snapshot.clone());
    if let Err(error) = refresh_recent_menu_with_retry(
        snapshot,
        || state.recent_files()?.list(),
        |recent_files| {
            state.set_native_recent_files(recent_files.clone());
            native_menu::refresh_app_menu(app, &state.native_menu_state())
                .map_err(|error| error.to_string())
        },
    ) {
        emit_app_feedback_error(app, error);
    }
}

#[tauri::command]
pub(crate) fn set_native_save_menu_enabled(
    enabled: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.set_native_save_menu_enabled(enabled);
    native_menu::set_save_menu_enabled(&app, enabled)
}

#[tauri::command]
pub(crate) fn set_native_theme_preference(
    selected_skin: String,
    follow_system: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.set_native_theme_preference(&selected_skin, follow_system)?;
    native_menu::refresh_app_menu(&app, &state.native_menu_state())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn set_native_locale_preference(
    mode: String,
    effective_locale: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.set_native_locale_preference(&mode, &effective_locale)?;
    native_menu::refresh_app_menu(&app, &state.native_menu_state())
        .map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ObservedContentEvidence {
    NotRequested,
    Compared {
        matches_expected: bool,
        bytes_read: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ObservedPath {
    Missing,
    Present {
        canonical_path: Option<std::path::PathBuf>,
        kind: ObservedPathKind,
        content: ObservedContentEvidence,
    },
}

enum RenamePathEvidence {
    Missing,
    Matches,
    Unexpected,
    ObservationFailed(String),
}

fn rename_path_evidence(
    filesystem: &impl FileSystemPort,
    path: &Path,
    is_file: bool,
) -> RenamePathEvidence {
    match filesystem.observe(path, None) {
        Ok(ObservedPath::Missing) => RenamePathEvidence::Missing,
        Ok(ObservedPath::Present {
            canonical_path: Some(canonical_path),
            kind,
            ..
        }) if canonical_path == path
            && kind
                == if is_file {
                    ObservedPathKind::File
                } else {
                    ObservedPathKind::Directory
                } =>
        {
            RenamePathEvidence::Matches
        }
        Ok(ObservedPath::Present { .. }) => RenamePathEvidence::Unexpected,
        Err(error) => RenamePathEvidence::ObservationFailed(error.to_string()),
    }
}

fn observe_rename_after_error(
    filesystem: &impl FileSystemPort,
    old_path: &Path,
    new_path: &Path,
    is_file: bool,
) -> RenameErrorObservation {
    let old = rename_path_evidence(filesystem, old_path, is_file);
    let new = rename_path_evidence(filesystem, new_path, is_file);
    match (&old, &new) {
        (RenamePathEvidence::Matches, RenamePathEvidence::Missing) => {
            RenameErrorObservation::ConfirmedNotCommitted
        }
        (RenamePathEvidence::Missing, RenamePathEvidence::Matches) => {
            RenameErrorObservation::ConfirmedCommitted
        }
        (RenamePathEvidence::Matches, RenamePathEvidence::Matches) => {
            RenameErrorObservation::Indeterminate {
                message: " Outcome observation found both old and new paths; the rename remains indeterminate."
                    .to_string(),
            }
        }
        (RenamePathEvidence::Missing, RenamePathEvidence::Missing) => {
            RenameErrorObservation::Indeterminate {
                message: " Outcome observation found neither old nor new path; the rename remains indeterminate."
                    .to_string(),
            }
        }
        _ => {
            let mut message = String::new();
            match old {
                RenamePathEvidence::Unexpected => message.push_str(
                    " Observation of old path did not match the expected kind and canonical identity.",
                ),
                RenamePathEvidence::ObservationFailed(error) => message
                    .push_str(&format!(" Observation of old path failed: {error}.")),
                RenamePathEvidence::Missing | RenamePathEvidence::Matches => {}
            }
            match new {
                RenamePathEvidence::Unexpected => message.push_str(
                    " Observation of new path did not match the expected kind and canonical identity.",
                ),
                RenamePathEvidence::ObservationFailed(error) => message
                    .push_str(&format!(" Observation of new path failed: {error}.")),
                RenamePathEvidence::Missing | RenamePathEvidence::Matches => {}
            }
            message.push_str(" The rename remains indeterminate.");
            RenameErrorObservation::Indeterminate { message }
        }
    }
}

fn observe_path(path: &Path, expected_bytes: Option<&[u8]>) -> std::io::Result<ObservedPath> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ObservedPath::Missing);
        }
        Err(error) => return Err(error),
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(ObservedPath::Present {
            canonical_path: None,
            kind: ObservedPathKind::Symlink,
            content: ObservedContentEvidence::NotRequested,
        });
    }

    let canonical_path = fs::canonicalize(path)?;
    let (kind, content) = if file_type.is_file() {
        let content = match expected_bytes {
            Some(expected) if canonical_path == path => {
                let observed = fs::read(&canonical_path)?;
                ObservedContentEvidence::Compared {
                    matches_expected: observed == expected,
                    bytes_read: observed.len(),
                }
            }
            Some(_) | None => ObservedContentEvidence::NotRequested,
        };
        (ObservedPathKind::File, content)
    } else if file_type.is_dir() {
        (
            ObservedPathKind::Directory,
            ObservedContentEvidence::NotRequested,
        )
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Observed path is not a file, directory, or symlink",
        ));
    };

    Ok(ObservedPath::Present {
        canonical_path: Some(canonical_path),
        kind,
        content,
    })
}

#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    const RENAME_EXCL: u32 = 0x0000_0004;

    unsafe extern "C" {
        fn renamex_np(
            from: *const std::ffi::c_char,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> std::ffi::c_int;
    }

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
    })?;
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    const AT_FDCWD: std::ffi::c_int = -100;
    const RENAME_NOREPLACE: std::ffi::c_uint = 1;

    unsafe extern "C" {
        fn renameat2(
            old_directory: std::ffi::c_int,
            old_path: *const std::ffi::c_char,
            new_directory: std::ffi::c_int,
            new_path: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
    })?;
    if unsafe {
        renameat2(
            AT_FDCWD,
            from.as_ptr(),
            AT_FDCWD,
            to.as_ptr(),
            RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    windows_handle_files::rename_no_replace(from, to)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn opened_directory_path(directory: &File) -> io::Result<PathBuf> {
    const F_GETPATH: std::ffi::c_int = 50;
    const PATH_BUFFER_SIZE: usize = 4096;

    unsafe extern "C" {
        fn fcntl(fd: std::ffi::c_int, command: std::ffi::c_int, ...) -> std::ffi::c_int;
    }

    let mut path = [0_u8; PATH_BUFFER_SIZE];
    if unsafe { fcntl(directory.as_raw_fd(), F_GETPATH, path.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let path = CStr::from_bytes_until_nul(&path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "opened path is not terminated"))?;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(
        path.to_bytes().to_vec(),
    )))
}

#[cfg(target_os = "linux")]
fn opened_directory_path(directory: &File) -> io::Result<PathBuf> {
    fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_file_without_following_links(path: &Path, bytes: &[u8]) -> io::Result<()> {
    unsafe extern "C" {
        fn openat(
            directory: std::ffi::c_int,
            path: *const std::ffi::c_char,
            flags: std::ffi::c_int,
            ...
        ) -> std::ffi::c_int;
    }

    #[cfg(target_os = "macos")]
    const O_CREAT: std::ffi::c_int = 0x0000_0200;
    #[cfg(target_os = "macos")]
    const O_EXCL: std::ffi::c_int = 0x0000_0800;
    #[cfg(target_os = "macos")]
    const O_NOFOLLOW: std::ffi::c_int = 0x0000_0100;
    #[cfg(target_os = "macos")]
    const O_CLOEXEC: std::ffi::c_int = 0x0100_0000;
    #[cfg(target_os = "linux")]
    const O_CREAT: std::ffi::c_int = 0x0000_0040;
    #[cfg(target_os = "linux")]
    const O_EXCL: std::ffi::c_int = 0x0000_0080;
    #[cfg(target_os = "linux")]
    const O_NOFOLLOW: std::ffi::c_int = 0x0002_0000;
    #[cfg(target_os = "linux")]
    const O_CLOEXEC: std::ffi::c_int = 0x0008_0000;
    const O_WRONLY: std::ffi::c_int = 0x0000_0001;

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let directory = File::open(parent)?;
    if !directory.metadata()?.is_dir() || opened_directory_path(&directory)? != parent {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "write parent changed after authorization",
        ));
    }
    let file_name = CString::new(file_name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name contains NUL"))?;
    let existing_flags = O_WRONLY | O_NOFOLLOW | O_CLOEXEC;
    let mut created = false;
    let mut descriptor =
        unsafe { openat(directory.as_raw_fd(), file_name.as_ptr(), existing_flags) };
    if descriptor == -1 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
        descriptor = unsafe {
            openat(
                directory.as_raw_fd(),
                file_name.as_ptr(),
                existing_flags | O_CREAT | O_EXCL,
                0o666_u32,
            )
        };
        if descriptor == -1 {
            return Err(io::Error::last_os_error());
        }
        created = true;
    }

    let mut file = unsafe { File::from_raw_fd(descriptor) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "write destination is not a regular file",
        ));
    }
    if !created {
        file.set_len(0)?;
    }
    file.write_all(bytes)
}

#[cfg(windows)]
mod windows_handle_files {
    use std::{
        ffi::{c_void, OsStr, OsString},
        fs::File,
        io::{self, Write},
        mem::{offset_of, size_of},
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::{AsRawHandle, FromRawHandle},
        },
        path::{Path, PathBuf},
        ptr::{null, null_mut},
    };

    use windows_sys::Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtCreateFile, FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT,
        },
    };
    use windows_sys::Win32::{
        Foundation::{
            RtlNtStatusToDosError, HANDLE, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, TRUE,
            UNICODE_STRING,
        },
        Globalization::{CompareStringOrdinal, CSTR_EQUAL},
        Storage::FileSystem::{
            CreateFileW, FileAttributeTagInfo, FileRenameInfo, GetFileInformationByHandleEx,
            GetFinalPathNameByHandleW, SetFileInformationByHandle, DELETE,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_RENAME_INFO_0, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_WRITE_DATA, OPEN_EXISTING, SYNCHRONIZE,
        },
        System::IO::IO_STATUS_BLOCK,
    };

    const DIRECTORY_SHARE_MODE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
    const REGULAR_FILE_OPEN_OPTIONS: u32 =
        FILE_OPEN_REPARSE_POINT | FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT;
    const ENTRY_OPEN_OPTIONS: u32 = FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;

    fn wide_nul(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    fn handle(file: &File) -> HANDLE {
        file.as_raw_handle() as HANDLE
    }

    fn invalid_input(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }

    fn permission_denied(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, message)
    }

    fn path_identity(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .map(|unit| {
                if unit == b'/' as u16 {
                    b'\\' as u16
                } else {
                    unit
                }
            })
            .collect()
    }

    fn paths_match(left: &Path, right: &Path) -> io::Result<bool> {
        let left = path_identity(left);
        let right = path_identity(right);
        let left_len = i32::try_from(left.len())
            .map_err(|_| invalid_input("opened path is too long to compare"))?;
        let right_len = i32::try_from(right.len())
            .map_err(|_| invalid_input("authorized path is too long to compare"))?;
        let comparison = unsafe {
            CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, TRUE)
        };
        if comparison == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(comparison == CSTR_EQUAL)
        }
    }

    fn opened_path(file: &File) -> io::Result<PathBuf> {
        let required = unsafe { GetFinalPathNameByHandleW(handle(file), null_mut(), 0, 0) };
        if required == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0_u16; required as usize + 1];
        let written = unsafe {
            GetFinalPathNameByHandleW(handle(file), buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if written as usize >= buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "opened path changed while it was queried",
            ));
        }
        buffer.truncate(written as usize);
        Ok(PathBuf::from(OsString::from_wide(&buffer)))
    }

    fn attribute_tag(file: &File) -> io::Result<FILE_ATTRIBUTE_TAG_INFO> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                handle(file),
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if succeeded == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(info)
        }
    }

    fn open_verified_parent(parent: &Path) -> io::Result<File> {
        let path = wide_nul(parent.as_os_str());
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                DIRECTORY_SHARE_MODE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let directory = unsafe { File::from_raw_handle(raw as _) };
        let attributes = attribute_tag(&directory)?.FileAttributes;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(permission_denied("parent directory is a reparse point"));
        }
        if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(invalid_input("parent path is not a directory"));
        }
        if !paths_match(&opened_path(&directory)?, parent)? {
            return Err(permission_denied(
                "parent directory changed after authorization",
            ));
        }
        Ok(directory)
    }

    fn relative_name(path: &Path) -> io::Result<Vec<u16>> {
        let name = path
            .file_name()
            .ok_or_else(|| invalid_input("path has no file name"))?;
        let name: Vec<u16> = name.encode_wide().collect();
        let byte_length = name
            .len()
            .checked_mul(size_of::<u16>())
            .ok_or_else(|| invalid_input("file name is too long"))?;
        if name.is_empty() || byte_length > u16::MAX as usize {
            return Err(invalid_input("file name is too long"));
        }
        Ok(name)
    }

    fn nt_error(status: i32) -> io::Error {
        let code = unsafe { RtlNtStatusToDosError(status) };
        io::Error::from_raw_os_error(code as i32)
    }

    fn nt_open_relative(
        directory: &File,
        name: &[u16],
        desired_access: u32,
        disposition: u32,
        open_options: u32,
    ) -> io::Result<File> {
        let unicode_name = UNICODE_STRING {
            Length: (name.len() * size_of::<u16>()) as u16,
            MaximumLength: (name.len() * size_of::<u16>()) as u16,
            Buffer: name.as_ptr() as *mut u16,
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: handle(directory),
            ObjectName: &unicode_name,
            Attributes: OBJ_CASE_INSENSITIVE,
            SecurityDescriptor: null(),
            SecurityQualityOfService: null(),
        };
        let mut io_status = IO_STATUS_BLOCK::default();
        let mut raw: HANDLE = null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut raw,
                desired_access,
                &attributes,
                &mut io_status,
                null(),
                FILE_ATTRIBUTE_NORMAL,
                DIRECTORY_SHARE_MODE,
                disposition,
                open_options,
                null(),
                0,
            )
        };
        if status < 0 {
            return Err(nt_error(status));
        }
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "NtCreateFile returned an invalid handle",
            ));
        }
        Ok(unsafe { File::from_raw_handle(raw as _) })
    }

    fn validate_regular_child(file: &File) -> io::Result<()> {
        let attributes = attribute_tag(file)?.FileAttributes;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid_input("file is a reparse point"));
        }
        if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(invalid_input("path is a directory"));
        }
        Ok(())
    }

    fn validate_rename_entry(file: &File) -> io::Result<()> {
        if attribute_tag(file)?.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid_input("rename entry is a reparse point"));
        }
        Ok(())
    }

    pub(super) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| invalid_input("path has no parent"))?;
        let name = relative_name(path)?;
        let directory = open_verified_parent(parent)?;
        let access = FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
        let (mut file, created) = match nt_open_relative(
            &directory,
            &name,
            access,
            FILE_OPEN,
            REGULAR_FILE_OPEN_OPTIONS,
        ) {
            Ok(file) => (file, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (
                nt_open_relative(
                    &directory,
                    &name,
                    access,
                    FILE_CREATE,
                    REGULAR_FILE_OPEN_OPTIONS,
                )?,
                true,
            ),
            Err(error) => return Err(error),
        };
        validate_regular_child(&file)?;
        if !created {
            file.set_len(0)?;
        }
        file.write_all(bytes)
    }

    fn destination_exists(directory: &File, name: &[u16]) -> io::Result<bool> {
        match nt_open_relative(
            directory,
            name,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_OPEN,
            ENTRY_OPEN_OPTIONS,
        ) {
            Ok(file) => {
                validate_rename_entry(&file)?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn rename_no_replace_with_precommit(
        from: &Path,
        to: &Path,
        precommit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let source_parent = from
            .parent()
            .ok_or_else(|| invalid_input("source path has no parent"))?;
        let destination_parent = to
            .parent()
            .ok_or_else(|| invalid_input("destination path has no parent"))?;
        let source_name = relative_name(from)?;
        let destination_name = relative_name(to)?;
        let source_directory = open_verified_parent(source_parent)?;
        let destination_directory = open_verified_parent(destination_parent)?;
        if destination_exists(&destination_directory, &destination_name)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "rename destination already exists",
            ));
        }
        let source = nt_open_relative(
            &source_directory,
            &source_name,
            DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_OPEN,
            ENTRY_OPEN_OPTIONS,
        )?;
        validate_rename_entry(&source)?;
        precommit()?;

        let name_bytes = destination_name.len() * size_of::<u16>();
        let buffer_bytes = offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
        let words = buffer_bytes.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let rename = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*rename).Anonymous = FILE_RENAME_INFO_0 {
                ReplaceIfExists: false,
            };
            (*rename).RootDirectory = handle(&destination_directory);
            (*rename).FileNameLength = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                destination_name.as_ptr(),
                std::ptr::addr_of_mut!((*rename).FileName).cast::<u16>(),
                destination_name.len(),
            );
        }
        let succeeded = unsafe {
            SetFileInformationByHandle(
                handle(&source),
                FileRenameInfo,
                rename.cast::<c_void>(),
                buffer_bytes as u32,
            )
        };
        if succeeded == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        rename_no_replace_with_precommit(from, to, || Ok(()))
    }

    #[cfg(test)]
    pub(super) fn rename_no_replace_with_hook(
        from: &Path,
        to: &Path,
        precommit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        rename_no_replace_with_precommit(from, to, precommit)
    }
}

#[cfg(windows)]
fn write_file_without_following_links(path: &Path, bytes: &[u8]) -> io::Result<()> {
    windows_handle_files::write(path, bytes)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn write_file_without_following_links(_path: &Path, _bytes: &[u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "nofollow handle-based writes are unavailable on this platform",
    ))
}

trait FileSystemPort {
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
    fn create_new(&self, path: &Path) -> std::io::Result<()>;
    fn create_new_with_contents(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.create_new(path)?;
        self.write(path, bytes)
    }
    fn create_dir(&self, path: &Path) -> std::io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()>;

    fn observe(&self, path: &Path, expected_bytes: Option<&[u8]>) -> std::io::Result<ObservedPath> {
        observe_path(path, expected_bytes)
    }
}

struct SystemFileSystemPort;

impl FileSystemPort for SystemFileSystemPort {
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        write_file_without_following_links(path, bytes)
    }

    fn create_new(&self, path: &Path) -> std::io::Result<()> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(drop)
    }

    fn create_new_with_contents(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)
    }

    fn create_dir(&self, path: &Path) -> std::io::Result<()> {
        fs::create_dir(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        rename_no_replace(from, to)
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
        fs::remove_dir_all(path)
    }
}

fn read_binary_file_bounded(
    path: &Path,
    source_limit: u64,
    limit_error: &str,
) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("Failed to open binary file {}: {error}", path.display()))?;
    if file
        .metadata()
        .map_err(|error| format!("Failed to inspect binary file {}: {error}", path.display()))?
        .len()
        > source_limit
    {
        return Err(limit_error.to_string());
    }

    let mut bytes = Vec::new();
    file.take(source_limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read binary file {}: {error}", path.display()))?;
    if bytes.len() as u64 > source_limit {
        return Err(limit_error.to_string());
    }
    Ok(bytes)
}

fn embedded_binary_open_response(
    kind: WorkspaceFileKind,
    path: &Path,
) -> Result<OpenFileResponse, String> {
    let bytes = match kind {
        WorkspaceFileKind::Pdf => read_binary_file_bounded(
            path,
            PDF_SOURCE_LIMIT_BYTES,
            "PDF source exceeds the 64 MiB limit",
        )?,
        WorkspaceFileKind::Docx => {
            let bytes = read_binary_file_bounded(
                path,
                DOCX_SOURCE_LIMIT_BYTES,
                "DOCX source exceeds the 32 MiB limit",
            )?;
            preflight_docx_zip(&bytes)?;
            bytes
        }
        _ => return Err("Workspace file does not require embedded binary bytes".to_string()),
    };

    Ok(OpenFileResponse {
        kind,
        path: path.to_string_lossy().to_string(),
        content_mode: ContentMode::Binary,
        content: None,
        mime_type: kind.mime_type(path),
        bytes_base64: Some(BASE64_STANDARD.encode(bytes)),
    })
}

pub(crate) fn open_authorized_file_response(
    file: std::path::PathBuf,
) -> Result<OpenFileResponse, String> {
    let kind = WorkspaceFileKind::classify(&file)
        .ok_or_else(|| "Selected file is not a supported preview file".to_string())?;
    if kind.requires_embedded_bytes() {
        embedded_binary_open_response(kind, &file)
    } else {
        kind.open_response(&file)
    }
}

fn allow_asset_preview_file(app: &AppHandle, file: &Path) -> Result<(), String> {
    app.asset_protocol_scope()
        .allow_file(file)
        .map_err(|err| format!("Failed to authorize preview assets: {err}"))
}

fn retry_asset_scope_sync(mut sync: impl FnMut() -> Result<(), String>) -> Result<(), String> {
    match sync() {
        Ok(()) => Ok(()),
        Err(_) => sync(),
    }
}

fn allow_asset_preview_file_with_retry(app: &AppHandle, file: &Path) -> Result<(), String> {
    retry_asset_scope_sync(|| allow_asset_preview_file(app, file))
}

fn allow_asset_preview_directory(app: &AppHandle, directory: &Path) -> Result<(), String> {
    app.asset_protocol_scope()
        .allow_directory(directory, true)
        .map_err(|err| format!("Failed to authorize preview assets: {err}"))
}

#[cfg(test)]
fn open_standalone_file_with_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    response: impl FnOnce(&Path) -> Result<OpenFileResponse, String>,
    transport: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<OpenFileResponse, String> {
    let (_, response) = state
        .file_authorization()
        .open_standalone_file(path, response, transport)?;
    Ok(response)
}

fn prepare_standalone_file_with_ports_inner(
    state: &AppState,
    owner_window: &str,
    path: impl AsRef<Path>,
    response: impl FnOnce(&Path) -> Result<OpenFileResponse, String>,
) -> Result<PreparedOpenFileResponse, String> {
    let file = normalize_existing_path(path)?;
    if !file.is_file() {
        return Err("Selected path is not a file".to_string());
    }
    let response = response(&file)?;
    let identifiers = state.recent_files()?.issue_open(owner_window, &file)?;
    Ok(PreparedOpenFileResponse {
        file: response,
        open_receipt: identifiers.open_receipt,
        commit_operation_id: identifiers.commit_operation_id,
    })
}

fn prepare_workspace_file_inner(
    state: &AppState,
    owner_window: &str,
    path: impl AsRef<Path>,
) -> Result<PreparedOpenFileResponse, String> {
    let file = ensure_authorized_existing_file_inner(state, path)?;
    let response = open_authorized_file_response(file.clone())?;
    let identifiers = state.recent_files()?.issue_open(owner_window, &file)?;
    Ok(PreparedOpenFileResponse {
        file: response,
        open_receipt: identifiers.open_receipt,
        commit_operation_id: identifiers.commit_operation_id,
    })
}

fn validate_workspace_entry_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Workspace entry name is empty".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Workspace entry name cannot contain path separators".into());
    }
    if matches!(name, "." | "..") || Path::new(name).components().count() != 1 {
        return Err("Workspace entry name is invalid".into());
    }
    Ok(name)
}

fn markdown_file_name(name: &str) -> Result<String, String> {
    let name = validate_workspace_entry_name(name)?;
    let path = Path::new(name);
    let normalized = if path.extension().is_none() {
        format!("{name}.md")
    } else {
        name.to_string()
    };
    if !WorkspaceFileKind::Markdown.allows_rename_to(Path::new(&normalized)) {
        return Err("Workspace file is not a Markdown/MDX file".into());
    }
    Ok(normalized)
}

fn excalidraw_file_name(name: &str) -> Result<String, String> {
    let name = validate_workspace_entry_name(name)?;
    let path = Path::new(name);
    let normalized = if path.extension().is_none() {
        format!("{name}.excalidraw")
    } else {
        name.to_string()
    };
    if !WorkspaceFileKind::Excalidraw.allows_rename_to(Path::new(&normalized)) {
        return Err("Workspace file is not an Excalidraw scene".into());
    }
    Ok(normalized)
}

fn new_workspace_file_spec(
    kind: WorkspaceFileKind,
    name: &str,
) -> Result<(String, String), String> {
    match kind {
        WorkspaceFileKind::Markdown => Ok((markdown_file_name(name)?, String::new())),
        WorkspaceFileKind::Excalidraw => Ok((
            excalidraw_file_name(name)?,
            default_excalidraw_scene().to_string(),
        )),
        _ => Err("Only Markdown and Excalidraw files can be created".into()),
    }
}

fn preview_file_name(current_kind: WorkspaceFileKind, name: &str) -> Result<String, String> {
    let name = validate_workspace_entry_name(name)?;
    if !current_kind.allows_rename_to(Path::new(name)) {
        return Err("Workspace file must keep the same supported file type".into());
    }
    Ok(name.to_string())
}

#[cfg(test)]
pub(crate) fn open_workspace_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<OpenFileResponse, String> {
    let file = ensure_authorized_existing_file_inner(state, path)?;
    if WorkspaceFileKind::classify(&file).is_none() {
        return Err("Workspace file is not a supported preview file".into());
    }
    let response = open_authorized_file_response(file.clone())?;
    authorize_workspace_file_inner(state, &file)?;
    Ok(response)
}

#[tauri::command]
pub(crate) async fn open_file_dialog(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<PreparedOpenFileResponse>, String> {
    let extensions = WorkspaceFileKind::all_extensions();
    let selected = app
        .dialog()
        .file()
        .add_filter("Documents and media", &extensions)
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|err| format!("Invalid selected file path: {err}"))?;
    prepare_standalone_file_with_ports_inner(&state, window.label(), path, |file| {
        open_authorized_file_response(file.to_path_buf())
    })
    .map(Some)
}

#[tauri::command]
pub(crate) fn open_workspace_file(
    path: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<PreparedOpenFileResponse, String> {
    prepare_workspace_file_inner(&state, window.label(), path)
}

#[tauri::command]
pub(crate) fn list_recent_files(state: State<'_, AppState>) -> Result<RecentFilesSnapshot, String> {
    state.recent_files()?.list()
}

#[tauri::command]
pub(crate) fn open_recent_file(
    entry_id: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<PreparedOpenFileResponse, String> {
    let prepared = state
        .recent_files()?
        .prepare_recent_open(window.label(), &entry_id, |path| {
            open_authorized_file_response(path.to_path_buf())
        });
    let (file, identifiers) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Ok(snapshot) = state.recent_files()?.list() {
                converge_recent_menu(&app, &state, &snapshot);
            } else {
                emit_app_feedback_error(&app, RECENT_MENU_SYNC_ERROR);
            }
            return Err(error);
        }
    };
    Ok(PreparedOpenFileResponse {
        file,
        open_receipt: identifiers.open_receipt,
        commit_operation_id: identifiers.commit_operation_id,
    })
}

#[tauri::command]
pub(crate) fn commit_recent_open(
    open_receipt: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<OpenCommitResult, String> {
    let mut asset_scope_error = None;
    let result = state.recent_files()?.commit_open_with_post_commit(
        &open_receipt,
        window.label(),
        state.file_authorization(),
        |file| {
            asset_scope_error = allow_asset_preview_file_with_retry(&app, file).err();
        },
    )?;
    if let Some(error) = asset_scope_error {
        emit_app_feedback_error(&app, error);
    }
    if let OpenCommitResult::Committed { recent_files } = &result {
        converge_recent_menu(&app, &state, recent_files);
    }
    Ok(result)
}

#[tauri::command]
pub(crate) fn get_open_commit_status(
    commit_operation_id: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<OpenCommitStatus, String> {
    state
        .recent_files()?
        .status(window.label(), &commit_operation_id)
}

#[tauri::command]
pub(crate) fn discard_open_receipt(
    open_receipt: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    state.recent_files()?.discard(window.label(), &open_receipt)
}

#[tauri::command]
pub(crate) fn remove_recent_file(
    entry_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RecentFilesSnapshot, String> {
    let snapshot = state.recent_files()?.remove(&entry_id)?;
    converge_recent_menu(&app, &state, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub(crate) fn clear_recent_files(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RecentFilesSnapshot, String> {
    let snapshot = state.recent_files()?.clear()?;
    converge_recent_menu(&app, &state, &snapshot);
    Ok(snapshot)
}

pub(crate) fn read_file_inner(state: &AppState, path: impl AsRef<Path>) -> Result<String, String> {
    let path = ensure_authorized_existing_file_inner(state, path)?;
    if !WorkspaceFileKind::classify(&path).is_some_and(WorkspaceFileKind::is_editable) {
        return Err("Only Markdown, HTML, and Excalidraw files can be read as text".into());
    }
    read_markdown_file(&path)
}

fn validate_editable_content(path: &Path, content: &str, action: &str) -> Result<(), String> {
    let kind = WorkspaceFileKind::classify(path)
        .ok_or_else(|| "Workspace file is not a supported editable document".to_string())?;
    if !kind.is_editable() {
        return Err(format!(
            "Only Markdown, HTML, and Excalidraw files can be {action}"
        ));
    }
    if kind == WorkspaceFileKind::Excalidraw {
        validate_excalidraw_scene(content)?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn read_file(path: String, state: State<'_, AppState>) -> Result<String, String> {
    read_file_inner(&state, path)
}

fn write_with_observation(
    path: &Path,
    content: &str,
    filesystem: &impl FileSystemPort,
) -> Result<(), String> {
    match filesystem.write(path, content.as_bytes()) {
        Ok(()) => Ok(()),
        Err(write_error) => {
            let confirmed_commit = matches!(
                filesystem.observe(path, Some(content.as_bytes())),
                Ok(ObservedPath::Present {
                    canonical_path: Some(observed_path),
                    kind: ObservedPathKind::File,
                    content: ObservedContentEvidence::Compared {
                        matches_expected: true,
                        bytes_read,
                    },
                }) if observed_path == path && bytes_read == content.len()
            );
            if confirmed_commit {
                return Ok(());
            }
            Err(format!(
                "Write may have partially changed the file after an error: Failed to write file: {write_error}. Reopen and inspect it before retrying."
            ))
        }
    }
}

fn write_file_with_preflight_and_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: &str,
    preflight: impl FnOnce(&Path) -> Result<(), String>,
    filesystem: &impl FileSystemPort,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    let outcome = match write_authorized_document_inner(state, path, preflight, |path| {
        write_with_observation(path, content, filesystem)
    }) {
        Ok(outcome) => outcome,
        Err(message) => return Ok(MutationOutcome::ConfirmedNotCommitted { message }),
    };

    Ok(match outcome {
        AuthorizedWriteOutcome::Committed(path) => MutationOutcome::ConfirmedCommitted {
            receipt: MutationCommitReceipt {
                committed: WorkspaceMutation {
                    path: path.to_string_lossy().to_string(),
                },
                workspace: SnapshotReceipt::NotApplicable,
            },
        },
        AuthorizedWriteOutcome::Indeterminate {
            path,
            recovery_message,
        } => MutationOutcome::Indeterminate {
            operation: crate::models::MutationKind::Write,
            paths: vec![path.to_string_lossy().to_string()],
            recovery_message,
        },
    })
}

fn write_file_with_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: &str,
    filesystem: &impl FileSystemPort,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    write_file_with_preflight_and_ports_inner(
        state,
        path,
        content,
        |path| validate_editable_content(path, content, "edited"),
        filesystem,
    )
}

pub(crate) fn write_file_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: &str,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    write_file_with_ports_inner(state, path, content, &SystemFileSystemPort)
}

#[tauri::command]
pub(crate) fn write_file(
    path: String,
    content: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    let write_token = normalize_file_for_write(&path).ok().and_then(|normalized| {
        state
            .active_document_watch()
            .begin_app_write(&normalized, content.as_bytes().to_vec())
    });
    let result = write_file_inner(&state, path, &content);
    if let Some(write_token) = write_token {
        let committed = matches!(&result, Ok(MutationOutcome::ConfirmedCommitted { .. }));
        state
            .active_document_watch()
            .settle_app_write_and_schedule(&app, write_token, committed);
    }
    result
}

pub(crate) fn save_as_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: String,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    save_as_with_ports_inner(state, path, &content, &SystemFileSystemPort)
}

pub(crate) fn save_as_for_kind_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: String,
    file_kind: Option<WorkspaceFileKind>,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    save_as_with_kind_and_ports_inner(state, path, &content, file_kind, &SystemFileSystemPort)
}

fn save_as_with_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: &str,
    filesystem: &impl FileSystemPort,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    save_as_with_kind_and_ports_inner(state, path, content, None, filesystem)
}

fn save_as_with_kind_and_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    content: &str,
    file_kind: Option<WorkspaceFileKind>,
    filesystem: &impl FileSystemPort,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    let outcome = match save_document_as_inner(
        state,
        path,
        |path| validate_save_as_content(path, content, file_kind),
        |path| write_with_observation(path, content, filesystem),
    ) {
        Ok(outcome) => outcome,
        Err(message) => return Ok(MutationOutcome::ConfirmedNotCommitted { message }),
    };

    Ok(match outcome {
        AuthorizedWriteOutcome::Committed(path) => MutationOutcome::ConfirmedCommitted {
            receipt: MutationCommitReceipt {
                committed: WorkspaceMutation {
                    path: path.to_string_lossy().to_string(),
                },
                workspace: SnapshotReceipt::NotApplicable,
            },
        },
        AuthorizedWriteOutcome::Indeterminate {
            path,
            recovery_message,
        } => MutationOutcome::Indeterminate {
            operation: crate::models::MutationKind::Write,
            paths: vec![path.to_string_lossy().to_string()],
            recovery_message,
        },
    })
}

fn validate_save_as_content(
    path: &Path,
    content: &str,
    file_kind: Option<WorkspaceFileKind>,
) -> Result<(), String> {
    if file_kind == Some(WorkspaceFileKind::Excalidraw)
        && WorkspaceFileKind::classify(path) != Some(WorkspaceFileKind::Excalidraw)
    {
        return Err("Excalidraw scenes must be saved with the .excalidraw extension".to_string());
    }
    validate_editable_content(path, content, "saved")
}

fn open_directory_with_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
    transport: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<WorkspaceSnapshot, String> {
    let (workspace, snapshot) = state
        .file_authorization()
        .open_workspace(path, snapshot, transport)?;
    snapshot.into_workspace_snapshot(&workspace)
}

fn open_persisted_directory_with_ports_inner(
    state: &AppState,
    path: impl AsRef<Path>,
    expected_root: &Path,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
    transport: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<WorkspaceSnapshot, String> {
    let (workspace, snapshot) = state
        .file_authorization()
        .open_workspace_at_canonical_root(path, expected_root, snapshot, transport)?;
    snapshot.into_workspace_snapshot(&workspace)
}

#[cfg(test)]
pub(crate) fn open_directory_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<WorkspaceSnapshot, String> {
    open_directory_with_ports_inner(state, path, capture_workspace_snapshot, |_| Ok(()))
}

fn canonical_persisted_workspace_root(path: &str) -> Result<std::path::PathBuf, String> {
    let raw = Path::new(path);
    let canonical = normalize_existing_path(raw)?;
    if raw != canonical || !canonical.is_dir() {
        return Err("Saved workspace root is no longer a canonical directory".to_string());
    }
    Ok(canonical)
}

fn canonical_persisted_active_file(
    state: &AppState,
    workspace_root: &Path,
    path: &str,
) -> Result<std::path::PathBuf, String> {
    let raw = Path::new(path);
    let canonical = normalize_existing_path(raw)?;
    if raw != canonical || !canonical.is_file() {
        return Err("Saved active file is no longer a canonical file".to_string());
    }
    if !path_is_under(&canonical, workspace_root) {
        return Err("Saved active file is outside the restored workspace".to_string());
    }
    if WorkspaceFileKind::classify(&canonical).is_none() {
        return Err("Saved active file is no longer supported".to_string());
    }
    let authorized = ensure_authorized_existing_file_inner(state, &canonical)?;
    if authorized != canonical {
        return Err("Saved active file changed while being restored".to_string());
    }
    Ok(canonical)
}

fn utf8_canonical_path(path: &Path, description: &str) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{description} is not valid UTF-8"))
}

fn clear_invalid_workspace_session(state: &AppState) -> Result<(), String> {
    state.workspace_session()?.clear()
}

fn clear_invalid_workspace_active_path(state: &AppState, record: &WorkspaceSessionRecord) {
    let _ = state
        .workspace_session()
        .and_then(|session| session.save(&record.without_active_path()));
}

fn validate_workspace_session_owner(owner: &str) -> Result<(), String> {
    if owner == "main" {
        Ok(())
    } else {
        Err("Only the main window can manage workspace session restoration".to_string())
    }
}

fn restore_workspace_session_with_ports_inner(
    state: &AppState,
    owner_window: &str,
    transport: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Option<WorkspaceSessionRestore>, String> {
    let Some(record) = state.workspace_session()?.load()? else {
        return Ok(None);
    };
    let workspace_root = match canonical_persisted_workspace_root(record.workspace_root()) {
        Ok(workspace_root) => workspace_root,
        Err(_) => {
            clear_invalid_workspace_session(state)?;
            return Ok(None);
        }
    };
    let workspace = match open_persisted_directory_with_ports_inner(
        state,
        &workspace_root,
        &workspace_root,
        capture_workspace_snapshot,
        transport,
    ) {
        Ok(workspace) => workspace,
        Err(_error) if canonical_persisted_workspace_root(record.workspace_root()).is_err() => {
            clear_invalid_workspace_session(state)?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };

    let active_file = match record.active_path() {
        None => None,
        Some(active_path) => match canonical_persisted_active_file(
            state,
            &workspace_root,
            active_path,
        )
        .and_then(|active_path| {
            let active_path_wire = active_path.to_string_lossy();
            if workspace
                .files
                .iter()
                .any(|file| file.path == active_path_wire.as_ref())
            {
                prepare_workspace_file_inner(state, owner_window, active_path)
            } else {
                Err("Saved active file is absent from the restored workspace snapshot".to_string())
            }
        }) {
            Ok(prepared) => Some(prepared),
            Err(_) => {
                clear_invalid_workspace_active_path(state, &record);
                None
            }
        },
    };
    Ok(Some(WorkspaceSessionRestore {
        workspace,
        active_file,
    }))
}

#[cfg(test)]
pub(crate) fn restore_workspace_session_inner(
    state: &AppState,
) -> Result<Option<WorkspaceSessionRestore>, String> {
    restore_workspace_session_with_ports_inner(state, "main", |_| Ok(()))
}

#[tauri::command]
pub(crate) fn restore_workspace_session(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<WorkspaceSessionRestore>, String> {
    validate_workspace_session_owner(window.label())?;
    restore_workspace_session_with_ports_inner(&state, window.label(), |root| {
        allow_asset_preview_directory(&app, root)
    })
}

fn persist_workspace_session_inner(
    state: &AppState,
    workspace_token: &str,
    workspace_root: &str,
    active_path: Option<&str>,
) -> Result<(), String> {
    let workspace_root = canonical_persisted_workspace_root(workspace_root)?;
    resolve_authorized_workspace_root_for_token_inner(state, workspace_token, &workspace_root)?;
    let active_path = active_path
        .map(|path| canonical_persisted_active_file(state, &workspace_root, path))
        .transpose()?;
    let record = WorkspaceSessionRecord::new(
        utf8_canonical_path(&workspace_root, "Workspace root")?,
        active_path
            .as_deref()
            .map(|path| utf8_canonical_path(path, "Active file"))
            .transpose()?,
    );
    state.workspace_session()?.save(&record)
}

#[tauri::command]
pub(crate) fn persist_workspace_session(
    workspace_token: String,
    workspace_root: String,
    active_path: Option<String>,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate_workspace_session_owner(window.label())?;
    persist_workspace_session_inner(
        &state,
        &workspace_token,
        &workspace_root,
        active_path.as_deref(),
    )
}

fn refresh_directory_with_snapshot_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<WorkspaceSnapshot, String> {
    let workspace =
        resolve_authorized_workspace_root_for_token_inner(state, workspace_token, path)?;
    snapshot(WorkspaceSnapshotSource::Authorized(&workspace))
        .and_then(|snapshot| snapshot.into_workspace_snapshot(&workspace))
}

pub(crate) fn refresh_directory_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
) -> Result<WorkspaceSnapshot, String> {
    refresh_directory_with_snapshot_inner(state, workspace_token, path, capture_workspace_snapshot)
}

fn capture_post_commit_workspace_receipt(
    state: &AppState,
    workspace: &AuthorizedWorkspace,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> SnapshotReceipt<WorkspaceSnapshot> {
    match snapshot(WorkspaceSnapshotSource::Authorized(workspace)) {
        Ok(snapshot) => match snapshot.into_workspace_snapshot(workspace) {
            Ok(snapshot) => match state
                .file_authorization()
                .ensure_workspace_is_current(workspace)
            {
                Ok(()) => SnapshotReceipt::Fresh { snapshot },
                Err(repair_reason) => SnapshotReceipt::Stale {
                    workspace_token: workspace.wire_token(),
                    repair_reason,
                },
            },
            Err(repair_reason) => SnapshotReceipt::Stale {
                workspace_token: workspace.wire_token(),
                repair_reason,
            },
        },
        Err(repair_reason) => SnapshotReceipt::Stale {
            workspace_token: workspace.wire_token(),
            repair_reason,
        },
    }
}

fn create_workspace_file_with_kind_and_ports_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
    kind: WorkspaceFileKind,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> {
    let (_, workspace) = match resolve_authorized_workspace_directory_for_token_inner(
        state,
        workspace_token,
        &parent_path,
    ) {
        Ok(value) => value,
        Err(message) => return Ok(MutationOutcome::ConfirmedNotCommitted { message }),
    };
    let (file_name, initial_content) = match new_workspace_file_spec(kind, name) {
        Ok(value) => value,
        Err(message) => return Ok(MutationOutcome::ConfirmedNotCommitted { message }),
    };
    let mut create_new_already_exists = false;
    let mut attempted_target = None;
    let created = state.file_authorization().create_workspace_file(
        &workspace,
        parent_path,
        &file_name,
        |target| {
            attempted_target = Some(target.to_path_buf());
            let result = if initial_content.is_empty() {
                filesystem.create_new(target)
            } else {
                filesystem.create_new_with_contents(target, initial_content.as_bytes())
            };
            result.map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    create_new_already_exists = true;
                    "Workspace entry already exists".to_string()
                } else {
                    format!("Failed to create file: {error}")
                }
            })
        },
    );
    let (workspace, file) = match created {
        Ok(created) => created,
        Err(message) if create_new_already_exists => {
            return Ok(MutationOutcome::ConfirmedNotCommitted { message });
        }
        Err(message) => {
            let Some(target) = attempted_target else {
                return Err(message);
            };
            let observation_note =
                match filesystem.observe(&target, Some(initial_content.as_bytes())) {
                    Ok(_) => String::new(),
                    Err(error) => format!(" Outcome observation failed: {error}."),
                };
            return Ok(MutationOutcome::Indeterminate {
                operation: crate::models::MutationKind::Create,
                paths: vec![target.to_string_lossy().to_string()],
                recovery_message: format!(
                    "File creation may have committed before an error: {message}. Refresh and inspect the workspace before retrying.{observation_note}"
                ),
            });
        }
    };
    let committed = OpenFileResponse {
        kind,
        path: file.into_path().to_string_lossy().to_string(),
        content_mode: ContentMode::Text,
        content: Some(initial_content),
        mime_type: None,
        bytes_base64: None,
    };
    let workspace_receipt = capture_post_commit_workspace_receipt(state, &workspace, snapshot);
    Ok(MutationOutcome::ConfirmedCommitted {
        receipt: MutationCommitReceipt {
            committed,
            workspace: workspace_receipt,
        },
    })
}

fn create_workspace_file_with_ports_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> {
    create_workspace_file_with_kind_and_ports_inner(
        state,
        workspace_token,
        parent_path,
        name,
        WorkspaceFileKind::Markdown,
        filesystem,
        snapshot,
    )
}

fn create_workspace_file_with_snapshot_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> {
    create_workspace_file_with_ports_inner(
        state,
        workspace_token,
        parent_path,
        name,
        &SystemFileSystemPort,
        snapshot,
    )
}

fn create_workspace_file_for_kind_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
    kind: WorkspaceFileKind,
) -> Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> {
    create_workspace_file_with_kind_and_ports_inner(
        state,
        workspace_token,
        parent_path,
        name,
        kind,
        &SystemFileSystemPort,
        capture_workspace_snapshot,
    )
}

pub(crate) fn create_workspace_file_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
) -> Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> {
    create_workspace_file_with_snapshot_inner(
        state,
        workspace_token,
        parent_path,
        name,
        capture_workspace_snapshot,
    )
}

fn create_workspace_directory_with_ports_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    let (_, workspace) = match resolve_authorized_workspace_directory_for_token_inner(
        state,
        workspace_token,
        &parent_path,
    ) {
        Ok(value) => value,
        Err(message) => return Ok(MutationOutcome::ConfirmedNotCommitted { message }),
    };
    let directory_name = match validate_workspace_entry_name(name) {
        Ok(value) => value,
        Err(message) => return Ok(MutationOutcome::ConfirmedNotCommitted { message }),
    };
    let mut create_dir_already_exists = false;
    let mut attempted_target = None;
    let created = state.file_authorization().create_workspace_directory(
        &workspace,
        parent_path,
        directory_name,
        |target| {
            attempted_target = Some(target.to_path_buf());
            filesystem.create_dir(target).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    create_dir_already_exists = true;
                }
                format!("Failed to create directory: {error}")
            })
        },
    );
    let (workspace, target) = match created {
        Ok(created) => created,
        Err(_message) if create_dir_already_exists => {
            return Ok(MutationOutcome::ConfirmedNotCommitted {
                message: "Workspace entry already exists".to_string(),
            });
        }
        Err(message) => {
            let Some(target) = attempted_target else {
                return Ok(MutationOutcome::ConfirmedNotCommitted { message });
            };
            let observation_note = match filesystem.observe(&target, None) {
                Ok(_) => String::new(),
                Err(error) => format!(" Outcome observation failed: {error}."),
            };
            return Ok(MutationOutcome::Indeterminate {
                operation: crate::models::MutationKind::Create,
                paths: vec![target.to_string_lossy().to_string()],
                recovery_message: format!(
                    "Directory creation may have committed before an error: {message}. Refresh and inspect the workspace before retrying.{observation_note}"
                ),
            });
        }
    };
    let committed = WorkspaceMutation {
        path: target.to_string_lossy().to_string(),
    };
    let workspace_receipt = capture_post_commit_workspace_receipt(state, &workspace, snapshot);
    Ok(MutationOutcome::ConfirmedCommitted {
        receipt: MutationCommitReceipt {
            committed,
            workspace: workspace_receipt,
        },
    })
}

fn create_workspace_directory_with_snapshot_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    create_workspace_directory_with_ports_inner(
        state,
        workspace_token,
        parent_path,
        name,
        &SystemFileSystemPort,
        snapshot,
    )
}

pub(crate) fn create_workspace_directory_inner(
    state: &AppState,
    workspace_token: &str,
    parent_path: impl AsRef<Path>,
    name: &str,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    create_workspace_directory_with_snapshot_inner(
        state,
        workspace_token,
        parent_path,
        name,
        capture_workspace_snapshot,
    )
}

fn finish_workspace_entry_relocation_outcome(
    state: &AppState,
    outcome: Result<AuthorizedRenameOutcome, String>,
    filesystem_called: bool,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) if !filesystem_called => {
            return Ok(MutationOutcome::ConfirmedNotCommitted { message });
        }
        Err(message) => return Err(message),
    };

    match outcome {
        AuthorizedRenameOutcome::ConfirmedNotCommitted { message } => {
            Ok(MutationOutcome::ConfirmedNotCommitted { message })
        }
        AuthorizedRenameOutcome::Committed(renamed) => {
            let committed = RenameWorkspaceEntryResponse {
                entry_kind: if renamed.is_file() {
                    "file"
                } else {
                    "directory"
                }
                .to_string(),
                old_path: renamed.old_path().to_string_lossy().to_string(),
                new_path: renamed.new_path().to_string_lossy().to_string(),
            };
            let workspace =
                capture_post_commit_workspace_receipt(state, renamed.workspace(), snapshot);
            Ok(MutationOutcome::ConfirmedCommitted {
                receipt: MutationCommitReceipt {
                    committed,
                    workspace,
                },
            })
        }
        AuthorizedRenameOutcome::RecoveryRequired {
            renamed,
            recovery_message,
        } => Ok(MutationOutcome::Indeterminate {
            operation: crate::models::MutationKind::Rename,
            paths: vec![
                renamed.old_path().to_string_lossy().to_string(),
                renamed.new_path().to_string_lossy().to_string(),
            ],
            recovery_message,
        }),
        AuthorizedRenameOutcome::Indeterminate {
            attempted,
            recovery_message,
            ..
        } => Ok(MutationOutcome::Indeterminate {
            operation: crate::models::MutationKind::Rename,
            paths: vec![
                attempted.old_path().to_string_lossy().to_string(),
                attempted.new_path().to_string_lossy().to_string(),
            ],
            recovery_message,
        }),
    }
}

fn rename_workspace_entry_with_preflight_and_ports_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    preflight: impl FnOnce(&Path, bool) -> Result<String, String>,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    let filesystem_called = std::cell::Cell::new(false);
    let outcome = rename_authorized_workspace_entry_inner(
        state,
        workspace_token,
        path,
        preflight,
        |entry, target| {
            filesystem_called.set(true);
            filesystem
                .rename(entry, target)
                .map_err(|err| format!("Failed to rename entry: {err}"))
        },
        |attempted| {
            observe_rename_after_error(
                filesystem,
                attempted.old_path(),
                attempted.new_path(),
                attempted.is_file(),
            )
        },
    );
    finish_workspace_entry_relocation_outcome(state, outcome, filesystem_called.get(), snapshot)
}

fn rename_workspace_entry_with_ports_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    new_name: &str,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    rename_workspace_entry_with_preflight_and_ports_inner(
        state,
        workspace_token,
        path,
        |entry, is_file| {
            if !is_file {
                return Ok(validate_workspace_entry_name(new_name)?.to_string());
            }
            let current_kind = WorkspaceFileKind::classify(entry).ok_or_else(|| {
                "Workspace file does not have a supported preview type".to_string()
            })?;
            if current_kind == WorkspaceFileKind::Markdown {
                markdown_file_name(new_name)
            } else {
                preview_file_name(current_kind, new_name)
            }
        },
        filesystem,
        snapshot,
    )
}

fn rename_workspace_entry_with_snapshot_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    new_name: &str,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    rename_workspace_entry_with_ports_inner(
        state,
        workspace_token,
        path,
        new_name,
        &SystemFileSystemPort,
        snapshot,
    )
}

pub(crate) fn rename_workspace_entry_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    new_name: &str,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    rename_workspace_entry_with_snapshot_inner(
        state,
        workspace_token,
        path,
        new_name,
        capture_workspace_snapshot,
    )
}

fn move_workspace_entry_with_ports_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    destination_parent_path: impl AsRef<Path>,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    let filesystem_called = std::cell::Cell::new(false);
    let outcome = move_authorized_workspace_entry_inner(
        state,
        workspace_token,
        path,
        destination_parent_path,
        |entry, target| {
            filesystem_called.set(true);
            filesystem
                .rename(entry, target)
                .map_err(|err| format!("Failed to move entry: {err}"))
        },
        |attempted| {
            observe_rename_after_error(
                filesystem,
                attempted.old_path(),
                attempted.new_path(),
                attempted.is_file(),
            )
        },
    );
    finish_workspace_entry_relocation_outcome(state, outcome, filesystem_called.get(), snapshot)
}

fn move_workspace_entry_with_snapshot_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    destination_parent_path: impl AsRef<Path>,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    move_workspace_entry_with_ports_inner(
        state,
        workspace_token,
        path,
        destination_parent_path,
        &SystemFileSystemPort,
        snapshot,
    )
}

pub(crate) fn move_workspace_entry_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    destination_parent_path: impl AsRef<Path>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    move_workspace_entry_with_snapshot_inner(
        state,
        workspace_token,
        path,
        destination_parent_path,
        capture_workspace_snapshot,
    )
}

fn delete_workspace_entry_with_ports_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    filesystem: &impl FileSystemPort,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<DeleteWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    let filesystem_called = std::cell::Cell::new(false);
    let requested_path = path.as_ref().to_path_buf();
    let outcome = delete_authorized_workspace_entry_inner(
        state,
        workspace_token,
        path,
        |entry, is_file| {
            filesystem_called.set(true);
            if is_file {
                filesystem
                    .remove_file(entry)
                    .map_err(|err| format!("Failed to delete file: {err}"))
            } else {
                filesystem
                    .remove_dir_all(entry)
                    .map_err(|err| format!("Failed to delete directory: {err}"))
            }
        },
        |entry| {
            filesystem
                .observe(entry, None)
                .map(|observed| match observed {
                    ObservedPath::Missing => DeleteFileObservation::Missing,
                    ObservedPath::Present { .. } => DeleteFileObservation::Present,
                })
                .map_err(|error| format!("Failed to inspect delete outcome: {error}"))
        },
    );
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) if !filesystem_called.get() => {
            return Ok(MutationOutcome::ConfirmedNotCommitted { message });
        }
        Err(message) => {
            let path = requested_path.to_string_lossy().to_string();
            return Ok(MutationOutcome::Indeterminate {
                operation: crate::models::MutationKind::Delete,
                paths: vec![path],
                recovery_message: format!(
                    "Delete may have partially changed the workspace after an error: {message}. Refresh and inspect the entry before retrying."
                ),
            });
        }
    };

    match outcome {
        AuthorizedDeleteOutcome::ConfirmedNotCommitted { message } => {
            Ok(MutationOutcome::ConfirmedNotCommitted { message })
        }
        AuthorizedDeleteOutcome::Committed(deleted) => {
            let committed = DeleteWorkspaceEntryResponse {
                deleted_path: deleted.deleted_path().to_string_lossy().to_string(),
            };
            let workspace =
                capture_post_commit_workspace_receipt(state, deleted.workspace(), snapshot);
            Ok(MutationOutcome::ConfirmedCommitted {
                receipt: MutationCommitReceipt {
                    committed,
                    workspace,
                },
            })
        }
        AuthorizedDeleteOutcome::RecoveryRequired {
            deleted,
            recovery_message,
        } => Ok(MutationOutcome::Indeterminate {
            operation: crate::models::MutationKind::Delete,
            paths: vec![deleted.deleted_path().to_string_lossy().to_string()],
            recovery_message,
        }),
        AuthorizedDeleteOutcome::Indeterminate {
            attempted,
            mut recovery_message,
        } => {
            let deleted_path = attempted.deleted_path().to_path_buf();
            if !attempted.is_file() {
                match filesystem.observe(&deleted_path, None) {
                    Ok(ObservedPath::Missing) => {
                        if let Err(error) = commit_indeterminate_delete_inner(state, &deleted_path)
                        {
                            recovery_message.push_str(&format!(
                                " Authorization reconciliation after observing a committed delete failed: {error}."
                            ));
                        } else {
                            let committed = DeleteWorkspaceEntryResponse {
                                deleted_path: deleted_path.to_string_lossy().to_string(),
                            };
                            let workspace = capture_post_commit_workspace_receipt(
                                state,
                                attempted.workspace(),
                                snapshot,
                            );
                            return Ok(MutationOutcome::ConfirmedCommitted {
                                receipt: MutationCommitReceipt {
                                    committed,
                                    workspace,
                                },
                            });
                        }
                    }
                    Ok(ObservedPath::Present { .. }) => {}
                    Err(error) => recovery_message
                        .push_str(&format!(" Delete outcome observation failed: {error}.")),
                }
            }
            Ok(MutationOutcome::Indeterminate {
                operation: crate::models::MutationKind::Delete,
                paths: vec![deleted_path.to_string_lossy().to_string()],
                recovery_message,
            })
        }
    }
}

fn delete_workspace_entry_with_snapshot_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
    snapshot: impl for<'a> FnOnce(
        WorkspaceSnapshotSource<'a>,
    ) -> Result<CapturedWorkspaceSnapshot, String>,
) -> Result<MutationOutcome<DeleteWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    delete_workspace_entry_with_ports_inner(
        state,
        workspace_token,
        path,
        &SystemFileSystemPort,
        snapshot,
    )
}

pub(crate) fn delete_workspace_entry_inner(
    state: &AppState,
    workspace_token: &str,
    path: impl AsRef<Path>,
) -> Result<MutationOutcome<DeleteWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    delete_workspace_entry_with_snapshot_inner(
        state,
        workspace_token,
        path,
        capture_workspace_snapshot,
    )
}

#[tauri::command]
pub(crate) async fn save_as_dialog(
    app: AppHandle,
    state: State<'_, AppState>,
    content: String,
    default_name: Option<String>,
    file_kind: Option<WorkspaceFileKind>,
) -> Result<Option<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>>, String> {
    let (filter_name, extensions) = if file_kind == Some(WorkspaceFileKind::Excalidraw) {
        ("Excalidraw", vec!["excalidraw"])
    } else {
        (
            "Markdown, HTML, and Excalidraw",
            WorkspaceFileKind::editable_extensions(),
        )
    };
    let mut dialog = app.dialog().file().add_filter(filter_name, &extensions);
    if let Some(default_name) = default_name.filter(|name| !name.trim().is_empty()) {
        dialog = dialog.set_file_name(default_name);
    }
    let Some(selected) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|err| format!("Invalid save path: {err}"))?;
    match file_kind {
        Some(file_kind) => save_as_for_kind_inner(&state, path, content, Some(file_kind)).map(Some),
        None => save_as_inner(&state, path, content).map(Some),
    }
}

#[tauri::command]
pub(crate) async fn open_directory_dialog(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<WorkspaceSnapshot>, String> {
    let Some(selected) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|err| format!("Invalid selected directory path: {err}"))?;
    let response =
        open_directory_with_ports_inner(&state, path, capture_workspace_snapshot, |root| {
            allow_asset_preview_directory(&app, root)
        })?;
    Ok(Some(response))
}

#[tauri::command]
pub(crate) fn refresh_directory(
    workspace_token: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<WorkspaceSnapshot, String> {
    refresh_directory_inner(&state, &workspace_token, path)
}

#[tauri::command]
pub(crate) fn create_workspace_file(
    workspace_token: String,
    parent_path: String,
    name: String,
    file_kind: Option<WorkspaceFileKind>,
    state: State<'_, AppState>,
) -> Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> {
    match file_kind.unwrap_or(WorkspaceFileKind::Markdown) {
        WorkspaceFileKind::Markdown => {
            create_workspace_file_inner(&state, &workspace_token, parent_path, &name)
        }
        kind => {
            create_workspace_file_for_kind_inner(&state, &workspace_token, parent_path, &name, kind)
        }
    }
}

#[tauri::command]
pub(crate) fn create_workspace_directory(
    workspace_token: String,
    parent_path: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<MutationOutcome<WorkspaceMutation, WorkspaceSnapshot>, String> {
    create_workspace_directory_inner(&state, &workspace_token, parent_path, &name)
}

#[tauri::command]
pub(crate) fn rename_workspace_entry(
    workspace_token: String,
    path: String,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    rename_workspace_entry_inner(&state, &workspace_token, path, &new_name)
}

#[tauri::command]
pub(crate) fn move_workspace_entry(
    workspace_token: String,
    path: String,
    destination_parent_path: String,
    state: State<'_, AppState>,
) -> Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    move_workspace_entry_inner(&state, &workspace_token, path, destination_parent_path)
}

#[tauri::command]
pub(crate) fn delete_workspace_entry(
    workspace_token: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<MutationOutcome<DeleteWorkspaceEntryResponse, WorkspaceSnapshot>, String> {
    delete_workspace_entry_inner(&state, &workspace_token, path)
}

pub(crate) fn read_workspace_image_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<String, String> {
    let path = ensure_authorized_existing_file_inner(state, path)?;
    if WorkspaceFileKind::classify(&path) != Some(WorkspaceFileKind::Image) {
        return Err("Workspace file is not a supported image".into());
    }
    let mime_type = WorkspaceFileKind::Image
        .mime_type(&path)
        .ok_or_else(|| "Workspace image has no supported MIME type".to_string())?;
    let bytes = read_binary_file_bounded(
        &path,
        IMAGE_SOURCE_LIMIT_BYTES,
        "Image source exceeds the 64 MiB limit",
    )?;
    Ok(format!(
        "data:{mime_type};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

#[tauri::command]
pub(crate) fn read_workspace_image(
    path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    read_workspace_image_inner(&state, path)
}

fn resolve_workspace_media_inner(
    state: &AppState,
    path: impl AsRef<Path>,
) -> Result<String, String> {
    let path = ensure_authorized_existing_file_inner(state, path)?;
    if !matches!(
        WorkspaceFileKind::classify(&path),
        Some(WorkspaceFileKind::Video | WorkspaceFileKind::Audio)
    ) {
        return Err("Workspace file is not supported audio or video".into());
    }
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub(crate) fn resolve_workspace_media(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let path = resolve_workspace_media_inner(&state, path)?;
    allow_asset_preview_file_with_retry(&app, Path::new(&path))?;
    Ok(path)
}

#[tauri::command]
pub(crate) fn resolve_markdown_image(
    current_file_path: String,
    workspace_root: Option<String>,
    image_src: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let path = resolve_relative_image_path_inner(
        &state,
        &current_file_path,
        workspace_root.as_deref(),
        &image_src,
    )?;
    allow_asset_preview_file_with_retry(&app, &path)?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::HashSet,
        path::PathBuf,
    };
    use tempfile::tempdir;

    use crate::path_auth::{is_authorized_image_path, normalize_existing_path};

    #[derive(Default)]
    struct ScriptedFileSystemPort {
        rename_calls: Cell<usize>,
        write_calls: Cell<usize>,
    }

    impl FileSystemPort for ScriptedFileSystemPort {
        fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
            self.write_calls.set(self.write_calls.get() + 1);
            Ok(())
        }

        fn create_new(&self, _path: &Path) -> std::io::Result<()> {
            unreachable!("scripted rename/write port must not create files")
        }

        fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
            unreachable!("scripted rename/write port must not create directories")
        }

        fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
            self.rename_calls.set(self.rename_calls.get() + 1);
            Ok(())
        }

        fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
            unreachable!("scripted rename/write port must not delete files")
        }

        fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
            unreachable!("scripted rename/write port must not delete directories")
        }
    }

    fn committed_rename_response(
        outcome: Result<MutationOutcome<RenameWorkspaceEntryResponse, WorkspaceSnapshot>, String>,
    ) -> RenameWorkspaceEntryResponse {
        match outcome.unwrap() {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt.committed,
            _ => panic!("expected confirmed committed rename outcome"),
        }
    }

    fn assert_confirmed_not_committed<T>(
        outcome: Result<MutationOutcome<T, WorkspaceSnapshot>, String>,
    ) {
        assert!(matches!(
            outcome.unwrap(),
            MutationOutcome::ConfirmedNotCommitted { .. }
        ));
    }

    const P2_PDF_SOURCE_LIMIT: u64 = 64 * 1024 * 1024;
    const P2_DOCX_SOURCE_LIMIT: u64 = 32 * 1024 * 1024;

    #[derive(Clone)]
    struct TestZipEntry {
        name: String,
        compression_method: u16,
        compressed_size: u64,
        uncompressed_size: u64,
    }

    impl TestZipEntry {
        fn new(name: impl Into<String>, compressed_size: u64, uncompressed_size: u64) -> Self {
            Self {
                name: name.into(),
                compression_method: 0,
                compressed_size,
                uncompressed_size,
            }
        }

        fn with_compression_method(mut self, compression_method: u16) -> Self {
            self.compression_method = compression_method;
            self
        }
    }

    fn append_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn append_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn zip64_size_fields(entry: &TestZipEntry) -> (u32, u32, Vec<u8>) {
        if entry.compressed_size <= u32::MAX as u64 && entry.uncompressed_size <= u32::MAX as u64 {
            return (
                entry.compressed_size as u32,
                entry.uncompressed_size as u32,
                Vec::new(),
            );
        }

        let mut extra = Vec::with_capacity(20);
        append_u16(&mut extra, 0x0001);
        append_u16(&mut extra, 16);
        extra.extend_from_slice(&entry.uncompressed_size.to_le_bytes());
        extra.extend_from_slice(&entry.compressed_size.to_le_bytes());
        (u32::MAX, u32::MAX, extra)
    }

    fn docx_zip(entries: &[TestZipEntry], valid_local_headers: bool) -> Vec<u8> {
        assert!(entries.len() <= u16::MAX as usize);
        let mut bytes = Vec::new();
        let mut local_offsets = Vec::with_capacity(entries.len());

        for entry in entries {
            let name = entry.name.as_bytes();
            assert!(name.len() <= u16::MAX as usize);
            local_offsets.push(bytes.len() as u32);
            append_u32(
                &mut bytes,
                if valid_local_headers {
                    0x0403_4b50
                } else {
                    0x0403_4b51
                },
            );
            append_u16(&mut bytes, 45);
            append_u16(&mut bytes, 0);
            append_u16(&mut bytes, entry.compression_method);
            append_u16(&mut bytes, 0);
            append_u16(&mut bytes, 0);
            append_u32(&mut bytes, 0);
            let (compressed_size, uncompressed_size, extra) = zip64_size_fields(entry);
            append_u32(&mut bytes, compressed_size);
            append_u32(&mut bytes, uncompressed_size);
            append_u16(&mut bytes, name.len() as u16);
            append_u16(&mut bytes, extra.len() as u16);
            bytes.extend_from_slice(name);
            bytes.extend_from_slice(&extra);
        }

        let central_offset = bytes.len() as u32;
        for (entry, local_offset) in entries.iter().zip(local_offsets) {
            let name = entry.name.as_bytes();
            let (compressed_size, uncompressed_size, extra) = zip64_size_fields(entry);
            append_u32(&mut bytes, 0x0201_4b50);
            append_u16(&mut bytes, 45);
            append_u16(&mut bytes, 45);
            append_u16(&mut bytes, 0);
            append_u16(&mut bytes, entry.compression_method);
            append_u16(&mut bytes, 0);
            append_u16(&mut bytes, 0);
            append_u32(&mut bytes, 0);
            append_u32(&mut bytes, compressed_size);
            append_u32(&mut bytes, uncompressed_size);
            append_u16(&mut bytes, name.len() as u16);
            append_u16(&mut bytes, extra.len() as u16);
            append_u16(&mut bytes, 0);
            append_u16(&mut bytes, 0);
            append_u16(&mut bytes, 0);
            append_u32(&mut bytes, 0);
            append_u32(&mut bytes, local_offset);
            bytes.extend_from_slice(name);
            bytes.extend_from_slice(&extra);
        }
        let central_size = bytes.len() as u32 - central_offset;
        append_u32(&mut bytes, 0x0605_4b50);
        append_u16(&mut bytes, 0);
        append_u16(&mut bytes, 0);
        append_u16(&mut bytes, entries.len() as u16);
        append_u16(&mut bytes, entries.len() as u16);
        append_u32(&mut bytes, central_size);
        append_u32(&mut bytes, central_offset);
        append_u16(&mut bytes, 0);
        bytes
    }

    fn minimal_docx_zip() -> Vec<u8> {
        docx_zip(&[TestZipEntry::new("[Content_Types].xml", 0, 0)], true)
    }

    fn assert_zip_central_directory_parses(bytes: &[u8], expected_entries: usize) {
        let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), expected_entries);
    }

    fn test_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let first = chunk[0];
            let second = chunk.get(1).copied().unwrap_or(0);
            let third = chunk.get(2).copied().unwrap_or(0);
            encoded.push(ALPHABET[(first >> 2) as usize] as char);
            encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
            encoded.push(if chunk.len() > 1 {
                ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
            } else {
                '='
            });
            encoded.push(if chunk.len() > 2 {
                ALPHABET[(third & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        encoded
    }

    fn assert_exact_binary_wire(path: &Path, source: &[u8], expected_kind: &str, mime: &str) {
        let state = AppState::default();
        authorize_directory_root_inner(&state, path.parent().unwrap().to_path_buf()).unwrap();
        let response = open_workspace_file_inner(&state, path).unwrap();
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": expected_kind,
                "path": path.canonicalize().unwrap().to_string_lossy(),
                "content_mode": "binary",
                "mime_type": mime,
                "bytes_base64": test_base64(source),
            })
        );
    }

    fn assert_docx_open_rejected_without_exact_grant(bytes: &[u8], expected_message: &str) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("hostile.docx");
        fs::write(&path, bytes).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, directory.path().to_path_buf()).unwrap();

        let error = open_workspace_file_inner(&state, &path).unwrap_err();

        assert!(
            error.contains(expected_message),
            "expected {error:?} to contain {expected_message:?}"
        );
        assert!(ensure_authorized_write_file_inner(&state, &path).is_err());
    }

    #[test]
    fn pdf_and_docx_open_responses_have_exact_binary_wire_shapes() {
        let directory = tempdir().unwrap();
        let pdf_path = directory.path().join("report.pdf");
        let docx_path = directory.path().join("report.docx");
        let image_path = directory.path().join("pixel.png");
        let pdf = b"%PDF-1.7\n%%EOF\n";
        let docx = minimal_docx_zip();
        fs::write(&pdf_path, pdf).unwrap();
        fs::write(&docx_path, &docx).unwrap();
        fs::write(&image_path, [0x89, b'P', b'N', b'G']).unwrap();

        assert_exact_binary_wire(&pdf_path, pdf, "pdf", "application/pdf");
        assert_exact_binary_wire(
            &docx_path,
            &docx,
            "docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        );

        let state = AppState::default();
        authorize_directory_root_inner(&state, directory.path().to_path_buf()).unwrap();
        let image =
            serde_json::to_value(open_workspace_file_inner(&state, &image_path).unwrap()).unwrap();
        assert!(image.get("bytes_base64").is_none());
        assert!(image.get("content").is_none());
    }

    #[test]
    fn workspace_snapshot_includes_pdf_and_docx_with_binary_kinds() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("a.pdf"), b"%PDF-1.7\n%%EOF\n").unwrap();
        fs::write(directory.path().join("b.docx"), minimal_docx_zip()).unwrap();
        fs::write(directory.path().join("ignored.zip"), b"PK").unwrap();

        let snapshot = open_directory_inner(&AppState::default(), directory.path()).unwrap();
        let entries = snapshot
            .files
            .iter()
            .map(|entry| {
                (
                    entry.relative_path.as_str(),
                    serde_json::to_value(entry.kind).unwrap(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            [
                ("a.pdf", serde_json::json!("pdf")),
                ("b.docx", serde_json::json!("docx")),
            ]
        );
    }

    #[test]
    fn pdf_and_docx_source_limits_reject_before_exact_grant_publication() {
        let directory = tempdir().unwrap();
        let cases = [
            ("oversized.pdf", P2_PDF_SOURCE_LIMIT + 1, "64 MiB"),
            ("oversized.docx", P2_DOCX_SOURCE_LIMIT + 1, "32 MiB"),
        ];

        for (name, length, expected_message) in cases {
            let path = directory.path().join(name);
            let file = fs::File::create(&path).unwrap();
            file.set_len(length).unwrap();
            let state = AppState::default();
            authorize_directory_root_inner(&state, directory.path().to_path_buf()).unwrap();

            let error = open_workspace_file_inner(&state, &path).unwrap_err();

            assert!(
                error.contains(expected_message),
                "expected {error:?} to contain {expected_message:?}"
            );
            assert!(ensure_authorized_write_file_inner(&state, &path).is_err());
        }
    }

    #[test]
    fn docx_preflight_rejects_malformed_and_truncated_central_directories() {
        assert_docx_open_rejected_without_exact_grant(
            b"PK\x03\x04truncated",
            "malformed or truncated",
        );

        let mut truncated = minimal_docx_zip();
        truncated.truncate(truncated.len() - 8);
        assert_docx_open_rejected_without_exact_grant(&truncated, "malformed or truncated");
    }

    #[test]
    fn docx_preflight_rejects_more_than_ten_thousand_entries() {
        let entries = (0..10_001)
            .map(|index| TestZipEntry::new(format!("word/item-{index}.xml"), 0, 0))
            .collect::<Vec<_>>();
        let bytes = docx_zip(&entries, true);

        assert_zip_central_directory_parses(&bytes, 10_001);
        assert_docx_open_rejected_without_exact_grant(&bytes, "10,000");
    }

    #[test]
    fn docx_preflight_rejects_declared_expansion_over_one_hundred_twenty_eight_mib() {
        let bytes = docx_zip(
            &[TestZipEntry::new(
                "word/document.xml",
                2 * 1024 * 1024,
                128 * 1024 * 1024 + 1,
            )],
            true,
        );

        assert_zip_central_directory_parses(&bytes, 1);
        assert_docx_open_rejected_without_exact_grant(&bytes, "128 MiB");
    }

    #[test]
    fn docx_preflight_rejects_zero_compressed_nonzero_output_and_ratio_above_one_hundred() {
        let zero_compressed = docx_zip(&[TestZipEntry::new("word/document.xml", 0, 1)], true);
        assert_zip_central_directory_parses(&zero_compressed, 1);
        assert_docx_open_rejected_without_exact_grant(&zero_compressed, "zero compressed bytes");

        let excessive_ratio = docx_zip(&[TestZipEntry::new("word/document.xml", 1, 101)], true);
        assert_zip_central_directory_parses(&excessive_ratio, 1);
        assert_docx_open_rejected_without_exact_grant(&excessive_ratio, "100:1");
    }

    #[test]
    fn docx_preflight_rejects_checked_size_overflow() {
        let bytes = docx_zip(
            &[
                TestZipEntry::new("word/one.bin", u64::MAX, u64::MAX),
                TestZipEntry::new("word/two.bin", u64::MAX, u64::MAX),
            ],
            true,
        );

        assert_zip_central_directory_parses(&bytes, 2);
        assert_docx_open_rejected_without_exact_grant(&bytes, "overflow");
    }

    #[test]
    fn docx_preflight_uses_central_directory_without_opening_entry_bodies() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("central-only.docx");
        let bytes = docx_zip(
            &[TestZipEntry::new("word/document.xml", 0, 0).with_compression_method(8)],
            true,
        );
        assert_zip_central_directory_parses(&bytes, 1);
        fs::write(&path, &bytes).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, directory.path().to_path_buf()).unwrap();

        let response = open_workspace_file_inner(&state, &path).unwrap();

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["kind"], "docx");
        assert_eq!(value["bytes_base64"], test_base64(&bytes));
    }

    #[test]
    fn binary_prepare_finishes_response_before_exact_grant_commit() {
        let directory = tempdir().unwrap();
        let recent_directory = tempdir().unwrap();
        let path = directory.path().join("report.docx");
        fs::write(&path, minimal_docx_zip()).unwrap();
        let state = AppState::default();
        state
            .initialize_recent_files(recent_directory.path().to_path_buf())
            .unwrap();
        authorize_directory_root_inner(&state, directory.path().to_path_buf()).unwrap();

        let prepared = prepare_workspace_file_inner(&state, "main", &path).unwrap();

        assert!(ensure_authorized_write_file_inner(&state, &path).is_err());
        let committed = state
            .recent_files()
            .unwrap()
            .commit_open(&prepared.open_receipt, "main", state.file_authorization())
            .unwrap();
        assert!(matches!(committed, OpenCommitResult::Committed { .. }));
        assert!(ensure_authorized_write_file_inner(&state, &path).is_ok());
    }

    #[test]
    fn pdf_and_docx_reject_text_read_write_save_as_and_rename() {
        let directory = tempdir().unwrap();
        let fixtures = [
            ("report.pdf", b"%PDF-1.7\n%%EOF\n".to_vec()),
            ("report.docx", minimal_docx_zip()),
        ];
        for (name, bytes) in &fixtures {
            fs::write(directory.path().join(name), bytes).unwrap();
        }
        let state = AppState::default();
        let workspace = open_directory_inner(&state, directory.path()).unwrap();

        for (name, original) in fixtures {
            let path = directory.path().join(name);
            open_workspace_file_inner(&state, &path).unwrap();

            assert_eq!(
                read_file_inner(&state, &path).unwrap_err(),
                "Only Markdown, HTML, and Excalidraw files can be read as text"
            );
            assert_confirmed_not_committed(write_file_inner(&state, &path, "replacement"));
            assert_confirmed_not_committed(save_as_inner(&state, &path, "replacement".to_string()));
            assert_confirmed_not_committed(rename_workspace_entry_inner(
                &state,
                &workspace.workspace_token,
                &path,
                &format!(
                    "renamed.{extension}",
                    extension = Path::new(name).extension().unwrap().to_string_lossy()
                ),
            ));
            assert_eq!(fs::read(&path).unwrap(), original);
            assert!(
                directory
                    .path()
                    .join(format!(
                        "renamed.{extension}",
                        extension = Path::new(name).extension().unwrap().to_string_lossy()
                    ))
                    .exists()
                    == false
            );
        }
    }

    #[test]
    fn recent_menu_refresh_reloads_and_retries_without_rolling_back_the_snapshot() {
        let initial = RecentFilesSnapshot { entries: vec![] };
        let reloaded = RecentFilesSnapshot {
            entries: vec![crate::models::RecentFileSummary {
                id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                display_name: "notes.md".to_string(),
            }],
        };
        let reload_calls = Cell::new(0);
        let refreshed = RefCell::new(Vec::new());

        let result = refresh_recent_menu_with_retry(
            &initial,
            || {
                reload_calls.set(reload_calls.get() + 1);
                Ok(reloaded.clone())
            },
            |snapshot| {
                refreshed.borrow_mut().push(snapshot.clone());
                if refreshed.borrow().len() == 1 {
                    Err("injected first refresh failure".to_string())
                } else {
                    Ok(())
                }
            },
        );

        assert!(result.is_ok());
        assert_eq!(reload_calls.get(), 1);
        assert_eq!(*refreshed.borrow(), [initial, reloaded]);
    }

    #[test]
    fn recent_menu_refresh_reports_failure_only_after_reload_and_retry() {
        let snapshot = RecentFilesSnapshot { entries: vec![] };
        let refresh_calls = Cell::new(0);

        let result = refresh_recent_menu_with_retry(
            &snapshot,
            || Ok(snapshot.clone()),
            |_| {
                refresh_calls.set(refresh_calls.get() + 1);
                Err("injected refresh failure".to_string())
            },
        );

        assert_eq!(result.unwrap_err(), RECENT_MENU_SYNC_ERROR);
        assert_eq!(refresh_calls.get(), 2);
    }

    #[test]
    fn asset_scope_sync_retries_once_before_success() {
        let attempts = Cell::new(0);

        let result = retry_asset_scope_sync(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err("injected asset scope failure".to_string())
            } else {
                Ok(())
            }
        });

        assert!(result.is_ok());
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn asset_scope_sync_reports_failure_only_after_retry() {
        let attempts = Cell::new(0);

        let result = retry_asset_scope_sync(|| {
            attempts.set(attempts.get() + 1);
            Err(format!("injected asset scope failure {}", attempts.get()))
        });

        assert_eq!(result, Err("injected asset scope failure 2".to_string()));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn workspace_file_open_authorizes_exact_file_for_later_write() {
        let dir = tempdir().unwrap();
        let doc = dir.path().join("doc.md");
        let sibling = dir.path().join("sibling.md");
        fs::write(&doc, "# doc").unwrap();
        fs::write(&sibling, "# sibling").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let response = open_workspace_file_inner(&state, &doc).unwrap();
        assert_eq!(response.content.as_deref(), Some("# doc"));
        assert_eq!(
            ensure_authorized_write_file_inner(&state, &doc).unwrap(),
            normalize_existing_path(&doc).unwrap()
        );
        assert!(ensure_authorized_write_file_inner(&state, &sibling).is_err());
    }

    #[test]
    fn workspace_text_decode_failure_publishes_no_exact_write_grant() {
        let dir = tempdir().unwrap();
        let document = dir.path().join("invalid.md");
        fs::write(&document, [0xff, 0xfe, 0xfd]).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let result = open_workspace_file_inner(&state, &document);

        assert!(result.is_err());
        assert!(ensure_authorized_write_file_inner(&state, &document).is_err());
    }

    #[test]
    fn standalone_open_uses_only_existing_parent_allow_pattern_before_internal_grant_publish() {
        use std::cell::RefCell;

        let dir = tempdir().unwrap();
        let document = dir.path().join("document.md");
        let sibling = dir.path().join("sibling.md");
        let asset = dir.path().join("asset.png");
        fs::write(&document, "# document").unwrap();
        fs::write(&sibling, "# sibling").unwrap();
        fs::write(&asset, b"png").unwrap();
        let canonical_document = document.canonicalize().unwrap();
        let canonical_parent = canonical_document.parent().unwrap().to_path_buf();
        let state = AppState::default();
        let events = RefCell::new(Vec::new());
        let transport_paths = RefCell::new(Vec::new());

        let response = open_standalone_file_with_ports_inner(
            &state,
            &document,
            |file| {
                events.borrow_mut().push("response");
                open_authorized_file_response(file.to_path_buf())
            },
            |parent| {
                assert_eq!(*events.borrow(), ["response"]);
                assert!(parent.is_dir());
                transport_paths.borrow_mut().push(parent.to_path_buf());
                events.borrow_mut().push("transport");
                Ok(())
            },
        )
        .unwrap();
        events.borrow_mut().push("published");

        assert_eq!(*events.borrow(), ["response", "transport", "published"]);
        assert_eq!(*transport_paths.borrow(), [canonical_parent]);
        assert_eq!(response.content.as_deref(), Some("# document"));
        assert!(ensure_authorized_write_file_inner(&state, &document).is_ok());
        assert!(ensure_authorized_existing_file_inner(&state, &sibling).is_err());
        assert!(is_authorized_image_path(&state, &asset.canonicalize().unwrap()).unwrap());

        let failed_state = AppState::default();
        let failed = open_standalone_file_with_ports_inner(
            &failed_state,
            &document,
            |file| open_authorized_file_response(file.to_path_buf()),
            |_| Err("injected transport failure".to_string()),
        );
        assert!(failed.is_err());
        assert!(ensure_authorized_existing_file_inner(&failed_state, &document).is_err());
        assert!(!is_authorized_image_path(&failed_state, &asset.canonicalize().unwrap()).unwrap());
    }

    #[test]
    fn unsupported_standalone_open_does_not_publish_or_request_transport_authority() {
        let dir = tempdir().unwrap();
        let document = dir.path().join("document.txt");
        let sibling = dir.path().join("sibling.md");
        fs::write(&document, "unsupported").unwrap();
        fs::write(&sibling, "# sibling").unwrap();
        let state = AppState::default();
        let transport_calls = Cell::new(0);

        let result = open_standalone_file_with_ports_inner(
            &state,
            &document,
            |file| open_authorized_file_response(file.to_path_buf()),
            |_| {
                transport_calls.set(transport_calls.get() + 1);
                Ok(())
            },
        );

        assert_eq!(
            result.unwrap_err(),
            "Selected file is not a supported preview file"
        );
        assert_eq!(transport_calls.get(), 0);
        assert!(ensure_authorized_existing_file_inner(&state, &document).is_err());
        assert!(ensure_authorized_existing_file_inner(&state, &sibling).is_err());
    }

    #[test]
    fn workspace_image_open_returns_preview_metadata_without_utf8_decoding() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("cover.png");
        fs::write(&image, [0x89, b'P', b'N', b'G']).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let response = open_workspace_file_inner(&state, &image).unwrap();

        assert_eq!(response.kind, WorkspaceFileKind::Image);
        assert_eq!(response.content, None);
        assert_eq!(response.mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn workspace_media_resolution_requires_existing_audio_or_video_authority() {
        let dir = tempdir().unwrap();
        let video = dir.path().join("clip.mp4");
        let image = dir.path().join("cover.png");
        fs::write(&video, b"video").unwrap();
        fs::write(&image, b"image").unwrap();
        let state = AppState::default();

        assert!(resolve_workspace_media_inner(&state, &video).is_err());
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();
        assert_eq!(
            resolve_workspace_media_inner(&state, &video).unwrap(),
            video.canonicalize().unwrap().to_string_lossy(),
        );
        assert!(resolve_workspace_media_inner(&state, &image).is_err());
    }

    #[test]
    fn workspace_html_open_returns_source_for_rendering() {
        let dir = tempdir().unwrap();
        let html = dir.path().join("index.html");
        fs::write(&html, "<!doctype html><h1>Hello</h1>").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let response = open_workspace_file_inner(&state, &html).unwrap();

        assert_eq!(response.kind, WorkspaceFileKind::Html);
        assert_eq!(
            response.content.as_deref(),
            Some("<!doctype html><h1>Hello</h1>")
        );
        assert_eq!(response.mime_type.as_deref(), Some("text/html"));
    }

    #[test]
    fn workspace_media_open_returns_preview_metadata_without_utf8_decoding() {
        let dir = tempdir().unwrap();
        let video = dir.path().join("clip.mp4");
        fs::write(&video, [0, 0, 0, 24, b'f', b't', b'y', b'p']).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let response = open_workspace_file_inner(&state, &video).unwrap();

        assert_eq!(response.kind, WorkspaceFileKind::Video);
        assert_eq!(response.content, None);
        assert_eq!(response.mime_type.as_deref(), Some("video/mp4"));
    }

    #[test]
    fn authorized_workspace_image_reads_as_a_typed_data_url() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("cover.png");
        let bytes = [0x89, b'P', b'N', b'G'];
        fs::write(&image, bytes).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();

        let data_url = read_workspace_image_inner(&state, &image).unwrap();

        assert_eq!(
            data_url,
            format!("data:image/png;base64,{}", test_base64(&bytes))
        );
    }

    #[test]
    fn workspace_image_read_requires_an_authorized_image_file() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("cover.png");
        let markdown = dir.path().join("notes.md");
        fs::write(&image, [0x89, b'P', b'N', b'G']).unwrap();
        fs::write(&markdown, "# Notes").unwrap();
        let state = AppState::default();

        assert!(read_workspace_image_inner(&state, &image).is_err());

        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();
        assert_eq!(
            read_workspace_image_inner(&state, &markdown).unwrap_err(),
            "Workspace file is not a supported image"
        );
    }

    #[test]
    fn markdown_write_command_rejects_an_opened_image() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("cover.png");
        let original = [0x89, b'P', b'N', b'G'];
        fs::write(&image, original).unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();
        open_workspace_file_inner(&state, &image).unwrap();

        let result = write_file_inner(&state, &image, "not an image");

        assert_confirmed_not_committed(result);
        assert_eq!(fs::read(&image).unwrap(), original);
    }

    #[test]
    fn read_file_rejects_non_editable_binary_content_before_utf8_decode() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("cover.png");
        fs::write(&image, "valid UTF-8 bytes inside a binary file").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();
        open_workspace_file_inner(&state, &image).unwrap();

        let result = read_file_inner(&state, &image);

        assert_eq!(
            result.unwrap_err(),
            "Only Markdown, HTML, and Excalidraw files can be read as text"
        );
    }

    #[test]
    fn write_command_updates_an_opened_html_document() {
        let dir = tempdir().unwrap();
        let html = dir.path().join("index.html");
        fs::write(&html, "<h1>Before</h1>").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();
        open_workspace_file_inner(&state, &html).unwrap();

        let outcome = write_file_inner(&state, &html, "<h1>After</h1>").unwrap();

        assert_eq!(fs::read_to_string(&html).unwrap(), "<h1>After</h1>");
        let MutationOutcome::ConfirmedCommitted { receipt } = outcome else {
            panic!("expected confirmed committed write outcome");
        };
        assert_eq!(
            Path::new(&receipt.committed.path),
            html.canonicalize().unwrap()
        );
        assert!(matches!(receipt.workspace, SnapshotReceipt::NotApplicable));
    }

    #[test]
    fn workspace_file_open_rejects_arbitrary_path_outside_authorized_root() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_doc = outside.path().join("outside.md");
        fs::write(&outside_doc, "# outside").unwrap();
        let state = AppState::default();
        authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();

        assert!(open_workspace_file_inner(&state, &outside_doc).is_err());
    }

    #[test]
    fn arbitrary_directory_refresh_without_prior_authorization_is_denied() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("doc.md"), "# doc").unwrap();
        let state = AppState::default();

        assert!(refresh_directory_inner(&state, "workspace-0", dir.path()).is_err());
    }

    #[test]
    fn workspace_mutations_create_rename_and_delete_entries_under_authorized_root() {
        let dir = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, dir.path()).unwrap();

        let created_file = match create_workspace_file_inner(
            &state,
            &opened.workspace_token,
            dir.path(),
            "draft.md",
        )
        .unwrap()
        {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt.committed,
            _ => panic!("expected confirmed committed outcome"),
        };
        assert_eq!(created_file.content.as_deref(), Some(""));
        assert!(Path::new(&created_file.path).is_file());
        assert!(ensure_authorized_write_file_inner(&state, &created_file.path).is_ok());

        let created_dir = match create_workspace_directory_inner(
            &state,
            &opened.workspace_token,
            dir.path(),
            "notes",
        )
        .unwrap()
        {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt.committed,
            _ => panic!("expected confirmed committed outcome"),
        };
        assert!(Path::new(&created_dir.path).is_dir());

        let renamed = committed_rename_response(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &created_file.path,
            "renamed.md",
        ));
        assert!(Path::new(&renamed.new_path).is_file());
        assert!(ensure_authorized_write_file_inner(&state, &renamed.new_path).is_ok());

        let deleted =
            delete_workspace_entry_inner(&state, &opened.workspace_token, &renamed.new_path)
                .unwrap();
        let receipt = match deleted {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        assert_eq!(receipt.committed.deleted_path, renamed.new_path);
        assert!(!Path::new(&receipt.committed.deleted_path).exists());
    }

    #[test]
    fn excalidraw_creation_open_write_and_save_as_require_valid_standard_scenes() {
        let directory = tempdir().unwrap();
        let state = AppState::default();
        let workspace = open_directory_inner(&state, directory.path()).unwrap();

        let created = match create_workspace_file_for_kind_inner(
            &state,
            &workspace.workspace_token,
            directory.path(),
            "architecture",
            WorkspaceFileKind::Excalidraw,
        )
        .unwrap()
        {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt.committed,
            _ => panic!("expected a committed Excalidraw creation"),
        };
        let initial_scene = default_excalidraw_scene();
        assert_eq!(created.kind, WorkspaceFileKind::Excalidraw);
        assert_eq!(created.content.as_deref(), Some(initial_scene));
        assert_eq!(
            Path::new(&created.path)
                .extension()
                .and_then(|value| value.to_str()),
            Some("excalidraw")
        );
        assert_eq!(fs::read_to_string(&created.path).unwrap(), initial_scene);

        let invalid_scene = r#"{"type":"excalidraw","version":2,"elements":[{"id":"shape","type":"rectangle","label":"private"}],"appState":{},"files":{}}"#;
        let before = fs::read_to_string(&created.path).unwrap();
        assert_confirmed_not_committed(write_file_inner(&state, &created.path, invalid_scene));
        assert_eq!(fs::read_to_string(&created.path).unwrap(), before);

        let invalid_destination = directory.path().join("invalid.excalidraw");
        assert_confirmed_not_committed(save_as_inner(
            &state,
            &invalid_destination,
            invalid_scene.to_string(),
        ));
        assert!(!invalid_destination.exists());

        let wrong_extension = directory.path().join("copy.md");
        assert_confirmed_not_committed(save_as_for_kind_inner(
            &state,
            &wrong_extension,
            initial_scene.to_string(),
            Some(WorkspaceFileKind::Excalidraw),
        ));
        assert!(!wrong_extension.exists());

        let valid_destination = directory.path().join("copy.excalidraw");
        let saved = save_as_for_kind_inner(
            &state,
            &valid_destination,
            initial_scene.to_string(),
            Some(WorkspaceFileKind::Excalidraw),
        )
        .unwrap();
        assert!(matches!(saved, MutationOutcome::ConfirmedCommitted { .. }));
        assert_eq!(
            fs::read_to_string(&valid_destination).unwrap(),
            initial_scene
        );

        let malformed = directory.path().join("malformed.excalidraw");
        fs::write(&malformed, invalid_scene).unwrap();
        assert!(open_workspace_file_inner(&state, &malformed).is_err());
    }

    #[test]
    fn delete_receipt_carries_the_selected_workspace_identity() {
        let outer = tempdir().unwrap();
        let inner = outer.path().join("inner");
        fs::create_dir(&inner).unwrap();
        let document = inner.join("draft.md");
        fs::write(&document, "# draft").unwrap();
        let state = AppState::default();
        let outer_snapshot = open_directory_inner(&state, outer.path()).unwrap();
        let inner_snapshot = open_directory_inner(&state, &inner).unwrap();

        assert_ne!(
            outer_snapshot.workspace_token,
            inner_snapshot.workspace_token
        );

        let canonical_document = document.canonicalize().unwrap();
        let deleted = delete_workspace_entry_inner(
            &state,
            &inner_snapshot.workspace_token,
            &canonical_document,
        )
        .unwrap();
        let receipt = match deleted {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };

        assert_eq!(
            receipt.committed.deleted_path,
            canonical_document.to_string_lossy()
        );
        match receipt.workspace {
            SnapshotReceipt::Fresh { snapshot } => {
                assert_eq!(snapshot.workspace_token, inner_snapshot.workspace_token);
                assert_eq!(snapshot.root, inner_snapshot.root);
            }
            _ => panic!("expected fresh selected-workspace receipt"),
        }
    }

    #[test]
    fn create_file_commit_survives_post_commit_snapshot_failure() {
        use crate::models::{MutationOutcome, SnapshotReceipt};

        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let target = workspace.path().join("draft.md");
        let snapshot_calls = std::cell::Cell::new(0);

        let outcome: Result<MutationOutcome<OpenFileResponse, WorkspaceSnapshot>, String> =
            create_workspace_file_with_snapshot_inner(
                &state,
                &opened.workspace_token,
                workspace.path(),
                "draft.md",
                |_source: WorkspaceSnapshotSource<'_>| {
                    snapshot_calls.set(snapshot_calls.get() + 1);
                    assert!(target.is_file());
                    assert_eq!(fs::read_to_string(&target).unwrap(), "");
                    assert!(ensure_authorized_write_file_inner(&state, &target).is_ok());
                    Err("injected post-commit snapshot failure".to_string())
                },
            );

        let receipt = match outcome.unwrap() {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        assert_eq!(snapshot_calls.get(), 1);
        assert_eq!(
            Path::new(&receipt.committed.path),
            target.canonicalize().unwrap()
        );
        assert_eq!(receipt.committed.content.as_deref(), Some(""));
        assert!(target.is_file());
        assert!(ensure_authorized_write_file_inner(&state, &target).is_ok());

        match receipt.workspace {
            SnapshotReceipt::Stale {
                workspace_token,
                repair_reason,
            } => {
                assert_eq!(workspace_token, opened.workspace_token);
                assert_eq!(repair_reason, "injected post-commit snapshot failure");
            }
            _ => panic!("expected stale workspace receipt"),
        }

        let unrelated_workspace = tempdir().unwrap();
        let unrelated_opened = open_directory_inner(&state, unrelated_workspace.path()).unwrap();
        assert_eq!(unrelated_opened.workspace_token, "workspace-1");

        let refreshed =
            refresh_directory_inner(&state, &opened.workspace_token, workspace.path()).unwrap();
        assert!(refreshed
            .files
            .iter()
            .any(|entry| entry.relative_path == "draft.md"));
    }

    #[test]
    fn create_file_already_exists_race_is_confirmed_not_committed_without_overwrite() {
        use serde_json::json;

        struct RacingCreateFileSystemPort {
            create_new_calls: Cell<usize>,
            created_path: RefCell<Option<PathBuf>>,
        }

        impl FileSystemPort for RacingCreateFileSystemPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("create-file race test must not use truncating write")
            }

            fn create_new(&self, path: &Path) -> std::io::Result<()> {
                self.create_new_calls.set(self.create_new_calls.get() + 1);
                *self.created_path.borrow_mut() = Some(path.to_path_buf());
                fs::write(path, b"racer-owned")?;
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map(drop)
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-file race test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("create-file race test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-file race test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-file race test must not delete directories")
            }

            fn observe(
                &self,
                _path: &Path,
                _expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                unreachable!("AlreadyExists is conclusive without observation")
            }
        }

        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let target = PathBuf::from(&opened.root).join("draft.md");
        let snapshot_calls = Cell::new(0);
        let filesystem = RacingCreateFileSystemPort {
            create_new_calls: Cell::new(0),
            created_path: RefCell::new(None),
        };

        let outcome = create_workspace_file_with_ports_inner(
            &state,
            &opened.workspace_token,
            workspace.path(),
            "draft.md",
            &filesystem,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run after a lost create race".to_string())
            },
        )
        .expect("AlreadyExists must be returned as a mutation outcome");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-not-committed",
                "message": "Workspace entry already exists",
            })
        );
        assert_eq!(filesystem.create_new_calls.get(), 1);
        assert_eq!(
            filesystem.created_path.borrow().as_deref(),
            Some(target.as_path())
        );
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(fs::read(&target).unwrap(), b"racer-owned");
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&target)
                .unwrap(),
            None
        );
        assert!(ensure_authorized_write_file_inner(&state, &target).is_err());
    }

    #[test]
    fn create_file_post_attempt_error_with_empty_file_is_indeterminate() {
        use serde_json::json;

        struct EmptyThenErrorFileSystemPort {
            create_new_calls: Cell<usize>,
            observe_calls: Cell<usize>,
            attempted_path: RefCell<Option<PathBuf>>,
        }

        impl FileSystemPort for EmptyThenErrorFileSystemPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("create-file error test must not use truncating write")
            }

            fn create_new(&self, path: &Path) -> std::io::Result<()> {
                self.create_new_calls.set(self.create_new_calls.get() + 1);
                *self.attempted_path.borrow_mut() = Some(path.to_path_buf());
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map(drop)?;
                Err(std::io::Error::other(
                    "injected post-attempt create failure",
                ))
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-file error test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("create-file error test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-file error test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-file error test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                self.observe_calls.set(self.observe_calls.get() + 1);
                assert_eq!(self.attempted_path.borrow().as_deref(), Some(path));
                assert_eq!(expected_bytes, Some([].as_slice()));
                observe_path(path, Some(&[]))
            }
        }

        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let target = PathBuf::from(&opened.root).join("draft.md");
        let authorization_before = state.file_authorization().state_fingerprint_for_test();
        let snapshot_calls = Cell::new(0);
        let filesystem = EmptyThenErrorFileSystemPort {
            create_new_calls: Cell::new(0),
            observe_calls: Cell::new(0),
            attempted_path: RefCell::new(None),
        };

        let outcome = create_workspace_file_with_ports_inner(
            &state,
            &opened.workspace_token,
            workspace.path(),
            "draft.md",
            &filesystem,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run after an indeterminate create".to_string())
            },
        )
        .expect("post-attempt create errors must be mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "create",
                "paths": [target.to_string_lossy()],
                "recovery_message": "File creation may have committed before an error: Failed to create file: injected post-attempt create failure. Refresh and inspect the workspace before retrying.",
            })
        );
        assert_eq!(filesystem.create_new_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert_eq!(
            filesystem.attempted_path.borrow().as_deref(),
            Some(target.as_path())
        );
        assert_eq!(snapshot_calls.get(), 0);
        let target_metadata = fs::metadata(&target).unwrap();
        assert!(target_metadata.is_file());
        assert_eq!(target_metadata.len(), 0);
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&target)
                .unwrap(),
            None
        );
        assert!(ensure_authorized_write_file_inner(&state, &target).is_err());

        let authorization_after = state.file_authorization().state_fingerprint_for_test();
        assert_eq!(
            authorization_after.rsplit_once(";counters=").unwrap().0,
            authorization_before.rsplit_once(";counters=").unwrap().0,
        );
    }

    #[test]
    fn create_directory_error_with_new_directory_is_indeterminate_and_ungranted() {
        use serde_json::json;

        struct DirectoryThenErrorFileSystemPort {
            create_dir_calls: Cell<usize>,
            observe_calls: Cell<usize>,
            attempted_path: RefCell<Option<PathBuf>>,
        }

        impl FileSystemPort for DirectoryThenErrorFileSystemPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("create-directory error test must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-directory error test must not create files")
            }

            fn create_dir(&self, path: &Path) -> std::io::Result<()> {
                self.create_dir_calls.set(self.create_dir_calls.get() + 1);
                *self.attempted_path.borrow_mut() = Some(path.to_path_buf());
                fs::create_dir(path)?;
                Err(std::io::Error::other(
                    "injected post-attempt directory create failure",
                ))
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("create-directory error test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-directory error test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("create-directory error test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                self.observe_calls.set(self.observe_calls.get() + 1);
                assert_eq!(self.attempted_path.borrow().as_deref(), Some(path));
                assert_eq!(expected_bytes, None);
                observe_path(path, None)
            }
        }

        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let target = PathBuf::from(&opened.root).join("notes");
        let authorization_before = state.file_authorization().state_fingerprint_for_test();
        let snapshot_calls = Cell::new(0);
        let filesystem = DirectoryThenErrorFileSystemPort {
            create_dir_calls: Cell::new(0),
            observe_calls: Cell::new(0),
            attempted_path: RefCell::new(None),
        };

        let outcome = create_workspace_directory_with_ports_inner(
            &state,
            &opened.workspace_token,
            workspace.path(),
            "notes",
            &filesystem,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run after an indeterminate create".to_string())
            },
        )
        .expect("post-attempt directory errors must be mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "create",
                "paths": [target.to_string_lossy()],
                "recovery_message": "Directory creation may have committed before an error: Failed to create directory: injected post-attempt directory create failure. Refresh and inspect the workspace before retrying.",
            })
        );
        assert_eq!(filesystem.create_dir_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert_eq!(
            filesystem.attempted_path.borrow().as_deref(),
            Some(target.as_path())
        );
        assert_eq!(snapshot_calls.get(), 0);
        assert!(fs::symlink_metadata(&target).unwrap().file_type().is_dir());
        assert!(fs::read_dir(&target).unwrap().next().is_none());
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&target)
                .unwrap(),
            None
        );
        assert_eq!(
            state.file_authorization().state_fingerprint_for_test(),
            authorization_before
        );
    }

    #[test]
    fn create_directory_already_exists_race_is_confirmed_not_committed() {
        use serde_json::json;

        struct AlreadyExistsDirectoryPort {
            create_dir_calls: Cell<usize>,
        }

        impl FileSystemPort for AlreadyExistsDirectoryPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("directory race test must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("directory race test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                self.create_dir_calls.set(self.create_dir_calls.get() + 1);
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "injected directory race",
                ))
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("directory race test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("directory race test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("directory race test must not delete directories")
            }

            fn observe(
                &self,
                _path: &Path,
                _expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                unreachable!("AlreadyExists is conclusive without observation")
            }
        }

        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let target = PathBuf::from(&opened.root).join("notes");
        let snapshot_calls = Cell::new(0);
        let filesystem = AlreadyExistsDirectoryPort {
            create_dir_calls: Cell::new(0),
        };

        let outcome = create_workspace_directory_with_ports_inner(
            &state,
            &opened.workspace_token,
            workspace.path(),
            "notes",
            &filesystem,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run after a lost directory race".to_string())
            },
        )
        .expect("AlreadyExists must be returned as a mutation outcome");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-not-committed",
                "message": "Workspace entry already exists",
            })
        );
        assert_eq!(filesystem.create_dir_calls.get(), 1);
        assert_eq!(snapshot_calls.get(), 0);
        assert!(!target.exists());
    }

    #[test]
    fn create_file_snapshot_is_stale_if_workspace_is_revoked_after_capture() {
        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let canonical_root = PathBuf::from(&opened.root);
        let target = canonical_root.join("draft.md");

        let outcome = create_workspace_file_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            &canonical_root,
            "draft.md",
            |source| {
                let captured = capture_workspace_snapshot(source)?;
                crate::path_auth::revoke_authorized_path_prefix_inner(&state, &canonical_root)?;
                Ok(captured)
            },
        )
        .unwrap();

        assert!(target.is_file());
        let receipt = match outcome {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        match receipt.workspace {
            SnapshotReceipt::Stale {
                workspace_token,
                repair_reason,
            } => {
                assert_eq!(workspace_token, opened.workspace_token);
                assert_eq!(repair_reason, "Workspace authorization is no longer active");
            }
            _ => panic!("expected stale workspace receipt"),
        }
    }

    #[test]
    fn create_mutations_reject_valid_token_for_a_different_authorized_workspace() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let state = AppState::default();
        let first_snapshot = open_directory_inner(&state, first.path()).unwrap();
        open_directory_inner(&state, second.path()).unwrap();
        let target = second.path().join("draft.md");
        let snapshot_calls = std::cell::Cell::new(0);

        let outcome = create_workspace_file_with_snapshot_inner(
            &state,
            &first_snapshot.workspace_token,
            second.path(),
            "draft.md",
            |source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                capture_workspace_snapshot(source)
            },
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            serde_json::json!({
                "status": "confirmed-not-committed",
                "message": "Directory is outside the selected workspace",
            })
        );
        assert_eq!(snapshot_calls.get(), 0);
        assert!(!target.exists());

        let directory_target = second.path().join("notes");
        let outcome = create_workspace_directory_with_snapshot_inner(
            &state,
            &first_snapshot.workspace_token,
            second.path(),
            "notes",
            |source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                capture_workspace_snapshot(source)
            },
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            serde_json::json!({
                "status": "confirmed-not-committed",
                "message": "Directory is outside the selected workspace",
            })
        );
        assert_eq!(snapshot_calls.get(), 0);
        assert!(!directory_target.exists());
    }

    #[test]
    fn create_directory_commit_survives_post_commit_snapshot_failure() {
        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let target = workspace.path().join("notes");
        let snapshot_calls = std::cell::Cell::new(0);

        let outcome = create_workspace_directory_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            workspace.path(),
            "notes",
            |_source: WorkspaceSnapshotSource<'_>| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                assert!(target.is_dir());
                assert!(refresh_directory_inner(&state, &opened.workspace_token, &target).is_err());
                Err("injected post-commit snapshot failure".to_string())
            },
        );

        let receipt = match outcome.unwrap() {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        assert_eq!(snapshot_calls.get(), 1);
        assert_eq!(
            Path::new(&receipt.committed.path),
            target.canonicalize().unwrap()
        );
        assert!(target.is_dir());
        match receipt.workspace {
            SnapshotReceipt::Stale {
                workspace_token,
                repair_reason,
            } => {
                assert_eq!(workspace_token, opened.workspace_token);
                assert_eq!(repair_reason, "injected post-commit snapshot failure");
            }
            _ => panic!("expected stale workspace receipt"),
        }

        let unrelated_workspace = tempdir().unwrap();
        let unrelated_opened = open_directory_inner(&state, unrelated_workspace.path()).unwrap();
        assert_eq!(unrelated_opened.workspace_token, "workspace-1");

        let refreshed =
            refresh_directory_inner(&state, &opened.workspace_token, workspace.path()).unwrap();
        assert!(refreshed
            .directories
            .iter()
            .any(|entry| entry.relative_path == "notes"));
    }

    #[test]
    fn rename_commit_survives_post_commit_snapshot_failure() {
        use serde_json::json;

        let outer = tempdir().unwrap();
        let inner = outer.path().join("inner");
        let source = inner.join("drafts");
        let old_document = source.join("index.html");
        let old_asset = source.join("asset.png");
        fs::create_dir_all(&source).unwrap();
        fs::write(&old_document, "<h1>before</h1>").unwrap();
        fs::write(&old_asset, b"png").unwrap();

        let state = AppState::default();
        open_directory_inner(&state, outer.path()).unwrap();
        let opened = open_directory_inner(&state, &inner).unwrap();
        open_standalone_file_with_ports_inner(
            &state,
            &old_document,
            |file| open_authorized_file_response(file.to_path_buf()),
            |_| Ok(()),
        )
        .unwrap();
        crate::html_preview_server::prepare_html_preview_inner(
            &state,
            &old_document,
            "<h1>preview</h1>",
        )
        .unwrap();

        let canonical_source = source.canonicalize().unwrap();
        let canonical_old_document = old_document.canonicalize().unwrap();
        let target = inner.canonicalize().unwrap().join("archive");
        let new_document = target.join("index.html");
        let new_asset = target.join("asset.png");
        let snapshot_calls = std::cell::Cell::new(0);

        let outcome = rename_workspace_entry_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            &canonical_source,
            "archive",
            |_source: WorkspaceSnapshotSource<'_>| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                assert!(!canonical_source.exists());
                assert!(target.is_dir());
                assert!(ensure_authorized_write_file_inner(&state, &new_document).is_ok());
                assert!(
                    ensure_authorized_write_file_inner(&state, &canonical_old_document).is_err()
                );
                assert!(is_authorized_image_path(&state, &new_asset).unwrap());
                assert!(!state
                    .html_preview_server
                    .site_documents()
                    .unwrap()
                    .contains(&canonical_old_document));
                assert!(state
                    .file_authorization()
                    .preview_lease_snapshot()
                    .unwrap()
                    .is_empty());
                Err("injected post-commit snapshot failure".to_string())
            },
        )
        .expect("committed rename must not become command failure");

        assert_eq!(snapshot_calls.get(), 1);
        assert!(!canonical_source.exists());
        assert!(target.is_dir());
        assert_eq!(
            fs::read_to_string(&new_document).unwrap(),
            "<h1>before</h1>"
        );
        write_file_inner(&state, &new_document, "<h1>after</h1>").unwrap();
        assert_eq!(fs::read_to_string(&new_document).unwrap(), "<h1>after</h1>");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-committed",
                "receipt": {
                    "committed": {
                        "entry_kind": "directory",
                        "old_path": canonical_source.to_string_lossy(),
                        "new_path": target.to_string_lossy(),
                    },
                    "workspace": {
                        "status": "stale",
                        "workspace_token": opened.workspace_token,
                        "repair_reason": "injected post-commit snapshot failure",
                    },
                },
            }),
        );

        let unrelated_workspace = tempdir().unwrap();
        let unrelated_opened = open_directory_inner(&state, unrelated_workspace.path()).unwrap();
        assert_eq!(unrelated_opened.workspace_token, "workspace-2");

        let refreshed = refresh_directory_inner(&state, &opened.workspace_token, &inner).unwrap();
        assert_eq!(refreshed.workspace_token, opened.workspace_token);
        assert!(refreshed
            .directories
            .iter()
            .any(|entry| entry.relative_path == "archive"));
        assert!(!refreshed
            .directories
            .iter()
            .any(|entry| entry.relative_path == "drafts"));
    }

    #[test]
    fn poisoned_authorization_fails_before_filesystem_mutation() {
        use serde_json::json;
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.html");
        fs::write(&source, "<h1>draft</h1>").unwrap();

        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &source).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(&state, &source, "<h1>preview</h1>")
            .unwrap();

        let canonical_source = source.canonicalize().unwrap();
        let target = canonical_source.with_file_name("renamed.html");
        let authorization_before = state.file_authorization().state_fingerprint_for_test();
        let sites_before = state.html_preview_server.site_documents().unwrap();

        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let _ = rename_authorized_workspace_entry_inner(
                &state,
                &opened.workspace_token,
                &canonical_source,
                |_, _| -> Result<String, String> {
                    panic!("injected authorization poison before filesystem mutation")
                },
                |_, _| panic!("filesystem mutation must not run while poisoning authorization"),
                |_| panic!("observation must not run while poisoning authorization"),
            );
        }));
        assert!(poisoned.is_err());

        let filesystem = ScriptedFileSystemPort::default();
        let snapshot_calls = Cell::new(0);
        let outcome = rename_workspace_entry_with_ports_inner(
            &state,
            &opened.workspace_token,
            &canonical_source,
            "renamed.html",
            &filesystem,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run for a pre-call failure".to_string())
            },
        )
        .expect("pre-call failures must be returned as mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-not-committed",
                "message": "Authorization state is poisoned",
            })
        );
        assert_eq!(filesystem.rename_calls.get(), 0);
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(
            fs::read_to_string(&canonical_source).unwrap(),
            "<h1>draft</h1>"
        );
        assert!(!target.exists());
        assert_eq!(
            state.file_authorization().state_fingerprint_for_test(),
            authorization_before
        );
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            sites_before
        );
    }

    #[test]
    fn injected_pre_call_rename_failure_preserves_disk_and_authorization() {
        use serde_json::json;

        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.html");
        fs::write(&source, "<h1>draft</h1>").unwrap();

        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &source).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(&state, &source, "<h1>preview</h1>")
            .unwrap();

        let canonical_source = source.canonicalize().unwrap();
        let target = canonical_source.with_file_name("renamed.html");
        let authorization_before = state.file_authorization().state_fingerprint_for_test();
        let sites_before = state.html_preview_server.site_documents().unwrap();
        let filesystem = ScriptedFileSystemPort::default();
        let snapshot_calls = Cell::new(0);

        let outcome = rename_workspace_entry_with_preflight_and_ports_inner(
            &state,
            &opened.workspace_token,
            &canonical_source,
            |entry, is_file| {
                assert_eq!(entry, canonical_source);
                assert!(is_file);
                Err("injected pre-call rename failure".to_string())
            },
            &filesystem,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run for a pre-call failure".to_string())
            },
        )
        .expect("pre-call failures must be returned as mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-not-committed",
                "message": "injected pre-call rename failure",
            })
        );
        assert_eq!(filesystem.rename_calls.get(), 0);
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(
            fs::read_to_string(&canonical_source).unwrap(),
            "<h1>draft</h1>"
        );
        assert!(!target.exists());
        assert_eq!(
            state.file_authorization().state_fingerprint_for_test(),
            authorization_before
        );
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            sites_before
        );
    }

    #[test]
    fn rename_os_error_layouts_are_classified_and_reconcile_affected_authority() {
        use serde_json::json;

        #[derive(Clone, Copy, Debug)]
        enum RenameErrorLayout {
            OldOnly,
            NewOnly,
            Both,
            Neither,
        }

        struct RenameErrorFileSystemPort {
            layout: RenameErrorLayout,
            rename_calls: Cell<usize>,
            renamed_paths: RefCell<Vec<(PathBuf, PathBuf)>>,
            observed_paths: RefCell<Vec<PathBuf>>,
        }

        impl FileSystemPort for RenameErrorFileSystemPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("rename error port must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename error port must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename error port must not create directories")
            }

            fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                self.rename_calls.set(self.rename_calls.get() + 1);
                self.renamed_paths
                    .borrow_mut()
                    .push((from.to_path_buf(), to.to_path_buf()));
                match self.layout {
                    RenameErrorLayout::OldOnly => {}
                    RenameErrorLayout::NewOnly => fs::rename(from, to)?,
                    RenameErrorLayout::Both => {
                        fs::create_dir(to)?;
                        fs::copy(from.join("index.html"), to.join("index.html"))?;
                    }
                    RenameErrorLayout::Neither => fs::remove_dir_all(from)?,
                }
                Err(std::io::Error::other(
                    "injected post-attempt rename failure",
                ))
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename error port must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename error port must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                crate::path_auth::lock_order_test_probe::assert_no_locks_held();
                assert!(expected_bytes.is_none());
                self.observed_paths.borrow_mut().push(path.to_path_buf());
                observe_path(path, None)
            }
        }

        for layout in [
            RenameErrorLayout::OldOnly,
            RenameErrorLayout::NewOnly,
            RenameErrorLayout::Both,
            RenameErrorLayout::Neither,
        ] {
            let workspace = tempdir().unwrap();
            let source = workspace.path().join("drafts");
            let target = workspace.path().join("archive");
            let unrelated = workspace.path().join("unrelated");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&target).unwrap();
            fs::create_dir(&unrelated).unwrap();
            let old_document = source.join("index.html");
            let new_document = target.join("index.html");
            let unrelated_document = unrelated.join("index.html");
            fs::write(&old_document, "old").unwrap();
            fs::write(&new_document, "new").unwrap();
            fs::write(&unrelated_document, "unrelated").unwrap();

            let state = AppState::default();
            let opened = open_directory_inner(&state, workspace.path()).unwrap();
            for document in [&old_document, &new_document, &unrelated_document] {
                open_standalone_file_with_ports_inner(
                    &state,
                    document,
                    |file| open_authorized_file_response(file.to_path_buf()),
                    |_| Ok(()),
                )
                .unwrap();
                crate::html_preview_server::prepare_html_preview_inner(&state, document, "preview")
                    .unwrap();
            }

            let canonical_workspace = workspace.path().canonicalize().unwrap();
            let canonical_source = source.canonicalize().unwrap();
            let canonical_target = target.canonicalize().unwrap();
            let canonical_old_document = old_document.canonicalize().unwrap();
            let canonical_new_document = new_document.canonicalize().unwrap();
            let canonical_unrelated = unrelated.canonicalize().unwrap();
            let canonical_unrelated_document = unrelated_document.canonicalize().unwrap();
            let all_sites = HashSet::from([
                canonical_old_document.clone(),
                canonical_new_document.clone(),
                canonical_unrelated_document.clone(),
            ]);

            assert_eq!(
                state.html_preview_server.site_documents().unwrap(),
                all_sites,
                "{layout:?}",
            );
            assert_eq!(
                state
                    .file_authorization()
                    .preview_lease_snapshot()
                    .unwrap()
                    .len(),
                3,
                "{layout:?}",
            );
            for document in [&canonical_old_document, &canonical_new_document] {
                assert_eq!(
                    state
                        .file_authorization()
                        .exact_write_grant_snapshot_for_test(document)
                        .unwrap(),
                    Some((GrantStatus::Active, 1)),
                    "{layout:?}",
                );
            }
            for directory in [&canonical_source, &canonical_target] {
                assert_eq!(
                    state
                        .file_authorization()
                        .internal_asset_grant_snapshot_for_test(directory)
                        .unwrap(),
                    Some((GrantStatus::Active, 2)),
                    "{layout:?}",
                );
            }
            let unrelated_exact_before = state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_unrelated_document)
                .unwrap();
            let unrelated_internal_before = state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_unrelated)
                .unwrap();
            assert_eq!(unrelated_exact_before, Some((GrantStatus::Active, 1)));
            assert_eq!(unrelated_internal_before, Some((GrantStatus::Active, 2)));

            fs::remove_dir_all(&canonical_target).unwrap();
            assert!(canonical_source.is_dir(), "{layout:?}");
            assert!(!canonical_target.exists(), "{layout:?}");

            let filesystem = RenameErrorFileSystemPort {
                layout,
                rename_calls: Cell::new(0),
                renamed_paths: RefCell::new(Vec::new()),
                observed_paths: RefCell::new(Vec::new()),
            };
            let snapshot_calls = Cell::new(0);
            let (outcome, lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
                rename_workspace_entry_with_ports_inner(
                    &state,
                    &opened.workspace_token,
                    &canonical_source,
                    "archive",
                    &filesystem,
                    |_source| {
                        snapshot_calls.set(snapshot_calls.get() + 1);
                        Err("snapshot must not run after a rename error".to_string())
                    },
                )
            });
            let outcome = outcome.expect("post-attempt rename errors must be mutation outcomes");

            let expected = match layout {
                RenameErrorLayout::OldOnly => json!({
                    "status": "confirmed-not-committed",
                    "message": "Failed to rename entry: injected post-attempt rename failure",
                }),
                RenameErrorLayout::NewOnly => json!({
                    "status": "confirmed-committed",
                    "receipt": {
                        "committed": {
                            "entry_kind": "directory",
                            "old_path": canonical_source.to_string_lossy(),
                            "new_path": canonical_target.to_string_lossy(),
                        },
                        "workspace": {
                            "status": "stale",
                            "workspace_token": opened.workspace_token,
                            "repair_reason": "snapshot must not run after a rename error",
                        },
                    },
                }),
                RenameErrorLayout::Both => json!({
                    "status": "indeterminate",
                    "operation": "rename",
                    "paths": [
                        canonical_source.to_string_lossy(),
                        canonical_target.to_string_lossy(),
                    ],
                    "recovery_message": "Rename may have partially changed the workspace after an error: Failed to rename entry: injected post-attempt rename failure. Refresh and inspect both paths before retrying. Outcome observation found both old and new paths; the rename remains indeterminate.",
                }),
                RenameErrorLayout::Neither => json!({
                    "status": "indeterminate",
                    "operation": "rename",
                    "paths": [
                        canonical_source.to_string_lossy(),
                        canonical_target.to_string_lossy(),
                    ],
                    "recovery_message": "Rename may have partially changed the workspace after an error: Failed to rename entry: injected post-attempt rename failure. Refresh and inspect both paths before retrying. Outcome observation found neither old nor new path; the rename remains indeterminate.",
                }),
            };
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                expected,
                "{layout:?}"
            );
            assert_eq!(filesystem.rename_calls.get(), 1, "{layout:?}");
            assert_eq!(
                *filesystem.renamed_paths.borrow(),
                vec![(canonical_source.clone(), canonical_target.clone())],
                "{layout:?}",
            );
            let observed_paths = filesystem.observed_paths.borrow();
            assert_eq!(
                *observed_paths,
                vec![canonical_source.clone(), canonical_target.clone()],
                "{layout:?}",
            );
            let authorization_acquired = lock_events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired
                    )
                })
                .count();
            let authorization_released = lock_events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased
                    )
                })
                .count();
            let html_acquired = lock_events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired
                    )
                })
                .count();
            let html_released = lock_events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased
                    )
                })
                .count();
            assert_eq!(authorization_acquired, authorization_released, "{layout:?}");
            assert_eq!(html_acquired, html_released, "{layout:?}");
            assert_eq!(
                snapshot_calls.get(),
                usize::from(matches!(layout, RenameErrorLayout::NewOnly)),
                "{layout:?}",
            );

            let (expect_old, expect_new) = match layout {
                RenameErrorLayout::OldOnly => (true, false),
                RenameErrorLayout::NewOnly => (false, true),
                RenameErrorLayout::Both => (true, true),
                RenameErrorLayout::Neither => (false, false),
            };
            for (path, expected) in [
                (canonical_source.as_path(), expect_old),
                (canonical_target.as_path(), expect_new),
            ] {
                match fs::symlink_metadata(path) {
                    Ok(metadata) => {
                        assert!(
                            expected,
                            "unexpected path for {layout:?}: {}",
                            path.display()
                        );
                        assert!(
                            metadata.file_type().is_dir(),
                            "present path is not a real directory for {layout:?}: {}",
                            path.display(),
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        assert!(!expected, "missing path for {layout:?}: {}", path.display());
                    }
                    Err(error) => panic!(
                        "cannot inspect path for {layout:?}: {}: {error}",
                        path.display(),
                    ),
                }
            }

            match layout {
                RenameErrorLayout::OldOnly => {
                    for document in [&canonical_old_document, &canonical_new_document] {
                        assert_eq!(
                            state
                                .file_authorization()
                                .exact_write_grant_snapshot_for_test(document)
                                .unwrap(),
                            Some((GrantStatus::Active, 1)),
                            "{layout:?}",
                        );
                    }
                    for directory in [&canonical_source, &canonical_target] {
                        assert_eq!(
                            state
                                .file_authorization()
                                .internal_asset_grant_snapshot_for_test(directory)
                                .unwrap(),
                            Some((GrantStatus::Active, 2)),
                            "{layout:?}",
                        );
                    }
                }
                RenameErrorLayout::NewOnly => {
                    assert_eq!(
                        state
                            .file_authorization()
                            .exact_write_grant_snapshot_for_test(&canonical_old_document)
                            .unwrap(),
                        None,
                        "{layout:?}",
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .exact_write_grant_snapshot_for_test(&canonical_new_document)
                            .unwrap(),
                        Some((GrantStatus::Active, 2)),
                        "{layout:?}",
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .internal_asset_grant_snapshot_for_test(&canonical_source)
                            .unwrap(),
                        None,
                        "{layout:?}",
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .internal_asset_grant_snapshot_for_test(&canonical_target)
                            .unwrap(),
                        Some((GrantStatus::Active, 2)),
                        "{layout:?}",
                    );
                }
                RenameErrorLayout::Both | RenameErrorLayout::Neither => {
                    for document in [&canonical_old_document, &canonical_new_document] {
                        assert_eq!(
                            state
                                .file_authorization()
                                .exact_write_grant_snapshot_for_test(document)
                                .unwrap(),
                            Some((GrantStatus::Suspended, 1)),
                            "{layout:?}",
                        );
                    }
                    for directory in [&canonical_source, &canonical_target] {
                        assert_eq!(
                            state
                                .file_authorization()
                                .internal_asset_grant_snapshot_for_test(directory)
                                .unwrap(),
                            Some((GrantStatus::Suspended, 1)),
                            "{layout:?}",
                        );
                    }
                }
            }
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(&canonical_unrelated_document)
                    .unwrap(),
                unrelated_exact_before,
                "{layout:?}",
            );
            assert_eq!(
                state
                    .file_authorization()
                    .internal_asset_grant_snapshot_for_test(&canonical_unrelated)
                    .unwrap(),
                unrelated_internal_before,
                "{layout:?}",
            );
            let expected_sites = if matches!(layout, RenameErrorLayout::OldOnly) {
                all_sites.clone()
            } else {
                HashSet::from([canonical_unrelated_document.clone()])
            };
            assert_eq!(
                state.html_preview_server.site_documents().unwrap(),
                expected_sites,
                "{layout:?}",
            );
            assert_eq!(
                state
                    .file_authorization()
                    .preview_lease_snapshot()
                    .unwrap()
                    .len(),
                if matches!(layout, RenameErrorLayout::OldOnly) {
                    3
                } else {
                    1
                },
                "{layout:?}",
            );
            let refreshed =
                refresh_directory_inner(&state, &opened.workspace_token, &canonical_workspace)
                    .unwrap();
            assert_eq!(
                refreshed.workspace_token, opened.workspace_token,
                "{layout:?}"
            );
        }
    }

    #[test]
    fn remove_file_error_layouts_distinguish_present_from_absent_commit() {
        use serde_json::json;

        #[derive(Clone, Copy, Debug)]
        enum RemoveFileErrorLayout {
            Present,
            Absent,
        }

        struct RemoveFileErrorFileSystemPort {
            layout: RemoveFileErrorLayout,
            remove_file_calls: Cell<usize>,
            removed_paths: RefCell<Vec<PathBuf>>,
            observed_paths: RefCell<Vec<PathBuf>>,
        }

        impl FileSystemPort for RemoveFileErrorFileSystemPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("remove-file error port must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("remove-file error port must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("remove-file error port must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("remove-file error port must not rename entries")
            }

            fn remove_file(&self, path: &Path) -> std::io::Result<()> {
                crate::path_auth::lock_order_test_probe::assert_authorization_held_without_html_sites();
                self.remove_file_calls.set(self.remove_file_calls.get() + 1);
                self.removed_paths.borrow_mut().push(path.to_path_buf());
                if matches!(self.layout, RemoveFileErrorLayout::Absent) {
                    fs::remove_file(path)?;
                }
                Err(std::io::Error::other(
                    "injected post-attempt file delete failure",
                ))
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("remove-file error port must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                crate::path_auth::lock_order_test_probe::assert_authorization_held_without_html_sites();
                assert!(expected_bytes.is_none());
                self.observed_paths.borrow_mut().push(path.to_path_buf());
                observe_path(path, None)
            }
        }

        for layout in [
            RemoveFileErrorLayout::Present,
            RemoveFileErrorLayout::Absent,
        ] {
            let outer = tempdir().unwrap();
            let workspace = outer.path().join("workspace");
            let shared = workspace.join("shared");
            fs::create_dir_all(&shared).unwrap();
            let affected = shared.join("affected.html");
            let unrelated = shared.join("unrelated.html");
            fs::write(&affected, "affected").unwrap();
            fs::write(&unrelated, "unrelated").unwrap();

            let state = AppState::default();
            let ancestor_opened = open_directory_inner(&state, outer.path()).unwrap();
            let opened = open_directory_inner(&state, &workspace).unwrap();
            for document in [&affected, &unrelated] {
                open_standalone_file_with_ports_inner(
                    &state,
                    document,
                    |file| open_authorized_file_response(file.to_path_buf()),
                    |_| Ok(()),
                )
                .unwrap();
                crate::html_preview_server::prepare_html_preview_inner(&state, document, "preview")
                    .unwrap();
            }

            let canonical_outer = outer.path().canonicalize().unwrap();
            let canonical_workspace = workspace.canonicalize().unwrap();
            let canonical_shared = shared.canonicalize().unwrap();
            let canonical_affected = affected.canonicalize().unwrap();
            let canonical_unrelated = unrelated.canonicalize().unwrap();
            let authorization_before = state.file_authorization().state_fingerprint_for_test();
            let sites_before = state.html_preview_server.site_documents().unwrap();
            let leases_before = state.file_authorization().preview_lease_snapshot().unwrap();

            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(&canonical_affected)
                    .unwrap(),
                Some((GrantStatus::Active, 1)),
                "{layout:?}",
            );
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(&canonical_unrelated)
                    .unwrap(),
                Some((GrantStatus::Active, 1)),
                "{layout:?}",
            );
            assert_eq!(
                state
                    .file_authorization()
                    .internal_asset_grant_snapshot_for_test(&canonical_shared)
                    .unwrap(),
                Some((GrantStatus::Active, 4)),
                "{layout:?}",
            );
            assert_eq!(
                sites_before,
                HashSet::from([canonical_affected.clone(), canonical_unrelated.clone()]),
                "{layout:?}",
            );
            assert_eq!(leases_before.len(), 2, "{layout:?}");

            let filesystem = RemoveFileErrorFileSystemPort {
                layout,
                remove_file_calls: Cell::new(0),
                removed_paths: RefCell::new(Vec::new()),
                observed_paths: RefCell::new(Vec::new()),
            };
            let snapshot_calls = Cell::new(0);
            let (outcome, lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
                delete_workspace_entry_with_ports_inner(
                    &state,
                    &opened.workspace_token,
                    &canonical_affected,
                    &filesystem,
                    |_source| {
                        crate::path_auth::lock_order_test_probe::assert_no_locks_held();
                        snapshot_calls.set(snapshot_calls.get() + 1);
                        Err("injected post-commit snapshot failure".to_string())
                    },
                )
            });
            let outcome =
                outcome.expect("post-attempt file delete errors must be mutation outcomes");

            let expected = match layout {
                RemoveFileErrorLayout::Present => json!({
                    "status": "confirmed-not-committed",
                    "message": "Failed to delete file: injected post-attempt file delete failure",
                }),
                RemoveFileErrorLayout::Absent => json!({
                    "status": "confirmed-committed",
                    "receipt": {
                        "committed": {
                            "deleted_path": canonical_affected.to_string_lossy(),
                        },
                        "workspace": {
                            "status": "stale",
                            "workspace_token": opened.workspace_token,
                            "repair_reason": "injected post-commit snapshot failure",
                        },
                    },
                }),
            };
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                expected,
                "{layout:?}"
            );
            assert_eq!(filesystem.remove_file_calls.get(), 1, "{layout:?}");
            assert_eq!(
                *filesystem.removed_paths.borrow(),
                vec![canonical_affected.clone()],
                "{layout:?}",
            );
            assert_eq!(
                *filesystem.observed_paths.borrow(),
                vec![canonical_affected.clone()],
                "{layout:?}",
            );

            match layout {
                RemoveFileErrorLayout::Present => {
                    assert_eq!(snapshot_calls.get(), 0);
                    assert_eq!(fs::read_to_string(&canonical_affected).unwrap(), "affected");
                    assert_eq!(
                        state.file_authorization().state_fingerprint_for_test(),
                        authorization_before,
                    );
                    assert_eq!(
                        state.html_preview_server.site_documents().unwrap(),
                        sites_before,
                    );
                    assert_eq!(
                        state.file_authorization().preview_lease_snapshot().unwrap(),
                        leases_before,
                    );
                    assert_eq!(
                        lock_events,
                        [
                            crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                            crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                        ],
                    );
                }
                RemoveFileErrorLayout::Absent => {
                    assert_eq!(snapshot_calls.get(), 1);
                    assert!(!canonical_affected.exists());
                    assert_eq!(
                        state
                            .file_authorization()
                            .exact_write_grant_snapshot_for_test(&canonical_affected)
                            .unwrap(),
                        None,
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .exact_write_grant_snapshot_for_test(&canonical_unrelated)
                            .unwrap(),
                        Some((GrantStatus::Active, 1)),
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .internal_asset_grant_snapshot_for_test(&canonical_shared)
                            .unwrap(),
                        Some((GrantStatus::Active, 2)),
                    );
                    assert_eq!(
                        state.html_preview_server.site_documents().unwrap(),
                        HashSet::from([canonical_unrelated.clone()]),
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .preview_lease_snapshot()
                            .unwrap()
                            .len(),
                        1,
                    );
                    assert_eq!(
                        lock_events,
                        [
                            crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                            crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                            crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                            crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                        ],
                    );
                }
            }

            resolve_authorized_workspace_root_for_token_inner(
                &state,
                &opened.workspace_token,
                &canonical_workspace,
            )
            .expect("selected workspace provenance must remain active");
            resolve_authorized_workspace_root_for_token_inner(
                &state,
                &ancestor_opened.workspace_token,
                &canonical_outer,
            )
            .expect("ancestor workspace provenance must remain active");
        }
    }

    #[test]
    fn partial_remove_dir_all_is_indeterminate() {
        use serde_json::json;

        struct PartialRemoveDirAllFileSystemPort {
            remove_dir_all_calls: Cell<usize>,
            removed_paths: RefCell<Vec<PathBuf>>,
            observed_paths: RefCell<Vec<PathBuf>>,
        }

        impl FileSystemPort for PartialRemoveDirAllFileSystemPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("partial directory delete port must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial directory delete port must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial directory delete port must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("partial directory delete port must not rename entries")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial directory delete port must not delete a single file")
            }

            fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
                crate::path_auth::lock_order_test_probe::assert_authorization_held_without_html_sites();
                self.remove_dir_all_calls
                    .set(self.remove_dir_all_calls.get() + 1);
                self.removed_paths.borrow_mut().push(path.to_path_buf());
                fs::remove_file(path.join("removed.html"))?;
                fs::remove_file(path.join("removed.png"))?;
                Err(std::io::Error::other(
                    "injected partial directory delete failure",
                ))
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                crate::path_auth::lock_order_test_probe::assert_no_locks_held();
                assert!(expected_bytes.is_none());
                self.observed_paths.borrow_mut().push(path.to_path_buf());
                observe_path(path, None)
            }
        }

        let outer = tempdir().unwrap();
        let workspace = outer.path().join("workspace");
        let target = workspace.join("target");
        let unrelated = workspace.join("unrelated");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        let removed_document = target.join("removed.html");
        let removed_image = target.join("removed.png");
        let retained_document = target.join("retained.html");
        let unrelated_document = unrelated.join("index.html");
        fs::write(&removed_document, "removed").unwrap();
        fs::write(&removed_image, b"removed-image").unwrap();
        fs::write(&retained_document, "retained").unwrap();
        fs::write(&unrelated_document, "unrelated").unwrap();

        let state = AppState::default();
        let ancestor_opened = open_directory_inner(&state, outer.path()).unwrap();
        let opened = open_directory_inner(&state, &workspace).unwrap();

        open_standalone_file_with_ports_inner(
            &state,
            &unrelated_document,
            |file| open_authorized_file_response(file.to_path_buf()),
            |_| Ok(()),
        )
        .unwrap();
        crate::html_preview_server::prepare_html_preview_inner(
            &state,
            &unrelated_document,
            "unrelated preview",
        )
        .unwrap();
        let unrelated_leases = state.file_authorization().preview_lease_snapshot().unwrap();
        assert_eq!(unrelated_leases.len(), 1);

        for document in [&removed_document, &retained_document] {
            open_standalone_file_with_ports_inner(
                &state,
                document,
                |file| open_authorized_file_response(file.to_path_buf()),
                |_| Ok(()),
            )
            .unwrap();
            crate::html_preview_server::prepare_html_preview_inner(
                &state,
                document,
                "affected preview",
            )
            .unwrap();
        }

        let canonical_outer = outer.path().canonicalize().unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();
        let canonical_target = target.canonicalize().unwrap();
        let canonical_unrelated = unrelated.canonicalize().unwrap();
        let canonical_removed_document = removed_document.canonicalize().unwrap();
        let canonical_removed_image = removed_image.canonicalize().unwrap();
        let canonical_retained_document = retained_document.canonicalize().unwrap();
        let canonical_unrelated_document = unrelated_document.canonicalize().unwrap();

        for document in [&canonical_removed_document, &canonical_retained_document] {
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(document)
                    .unwrap(),
                Some((GrantStatus::Active, 1)),
            );
        }
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_target)
                .unwrap(),
            Some((GrantStatus::Active, 4)),
        );
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_unrelated_document)
                .unwrap(),
            Some((GrantStatus::Active, 1)),
        );
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_unrelated)
                .unwrap(),
            Some((GrantStatus::Active, 2)),
        );
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            HashSet::from([
                canonical_removed_document.clone(),
                canonical_retained_document.clone(),
                canonical_unrelated_document.clone(),
            ]),
        );
        assert_eq!(
            state
                .file_authorization()
                .preview_lease_snapshot()
                .unwrap()
                .len(),
            3,
        );

        let filesystem = PartialRemoveDirAllFileSystemPort {
            remove_dir_all_calls: Cell::new(0),
            removed_paths: RefCell::new(Vec::new()),
            observed_paths: RefCell::new(Vec::new()),
        };
        let snapshot_calls = Cell::new(0);
        let (outcome, lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
            delete_workspace_entry_with_ports_inner(
                &state,
                &opened.workspace_token,
                &canonical_target,
                &filesystem,
                |_source| {
                    crate::path_auth::lock_order_test_probe::assert_no_locks_held();
                    snapshot_calls.set(snapshot_calls.get() + 1);
                    Err("snapshot must not run after a partial directory delete".to_string())
                },
            )
        });
        let outcome = outcome.expect("partial directory delete errors must be mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "delete",
                "paths": [canonical_target.to_string_lossy()],
                "recovery_message": "Directory deletion may have partially changed the workspace after an error: Failed to delete directory: injected partial directory delete failure. Refresh and inspect the directory before retrying.",
            }),
        );
        assert_eq!(filesystem.remove_dir_all_calls.get(), 1);
        assert_eq!(
            *filesystem.removed_paths.borrow(),
            vec![canonical_target.clone()],
        );
        assert_eq!(
            *filesystem.observed_paths.borrow(),
            vec![canonical_target.clone()],
        );
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(
            lock_events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
            ],
        );

        assert!(canonical_target.is_dir());
        assert!(!canonical_removed_document.exists());
        assert!(!canonical_removed_image.exists());
        assert_eq!(
            fs::read_to_string(&canonical_retained_document).unwrap(),
            "retained",
        );
        for document in [&canonical_removed_document, &canonical_retained_document] {
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(document)
                    .unwrap(),
                Some((GrantStatus::Suspended, 1)),
            );
        }
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_target)
                .unwrap(),
            Some((GrantStatus::Suspended, 2)),
        );
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_unrelated_document)
                .unwrap(),
            Some((GrantStatus::Active, 1)),
        );
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_unrelated)
                .unwrap(),
            Some((GrantStatus::Active, 2)),
        );
        assert!(ensure_authorized_write_file_inner(&state, &canonical_retained_document).is_err());
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            HashSet::from([canonical_unrelated_document]),
        );
        assert_eq!(
            state.file_authorization().preview_lease_snapshot().unwrap(),
            unrelated_leases,
        );
        resolve_authorized_workspace_root_for_token_inner(
            &state,
            &opened.workspace_token,
            &canonical_workspace,
        )
        .expect("selected workspace provenance must remain active");
        resolve_authorized_workspace_root_for_token_inner(
            &state,
            &ancestor_opened.workspace_token,
            &canonical_outer,
        )
        .expect("ancestor workspace provenance must remain active");
    }

    #[test]
    fn remove_dir_all_error_after_complete_delete_is_confirmed_committed() {
        struct RemoveDirAllThenErrorPort {
            remove_dir_all_calls: Cell<usize>,
            observe_calls: Cell<usize>,
        }

        impl FileSystemPort for RemoveDirAllThenErrorPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("directory delete test must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("directory delete test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("directory delete test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("directory delete test must not rename entries")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("directory delete test must not delete a single file")
            }

            fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
                crate::path_auth::lock_order_test_probe::assert_authorization_held_without_html_sites();
                self.remove_dir_all_calls
                    .set(self.remove_dir_all_calls.get() + 1);
                fs::remove_dir_all(path)?;
                Err(std::io::Error::other(
                    "injected post-commit directory delete failure",
                ))
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                crate::path_auth::lock_order_test_probe::assert_no_locks_held();
                assert!(expected_bytes.is_none());
                self.observe_calls.set(self.observe_calls.get() + 1);
                observe_path(path, None)
            }
        }

        let workspace = tempdir().unwrap();
        let target = workspace.path().join("target");
        let document = target.join("index.html");
        fs::create_dir(&target).unwrap();
        fs::write(&document, "<h1>target</h1>").unwrap();

        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &document).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(&state, &document, "preview")
            .unwrap();
        let canonical_target = target.canonicalize().unwrap();
        let canonical_document = document.canonicalize().unwrap();
        let filesystem = RemoveDirAllThenErrorPort {
            remove_dir_all_calls: Cell::new(0),
            observe_calls: Cell::new(0),
        };

        let (outcome, lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
            delete_workspace_entry_with_ports_inner(
                &state,
                &opened.workspace_token,
                &canonical_target,
                &filesystem,
                capture_workspace_snapshot,
            )
        });
        let outcome = outcome.expect("observed complete delete must be a mutation outcome");
        let receipt = match outcome {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed delete outcome"),
        };

        assert_eq!(
            receipt.committed.deleted_path,
            canonical_target.to_string_lossy()
        );
        let SnapshotReceipt::Fresh { snapshot } = receipt.workspace else {
            panic!("expected a fresh workspace snapshot");
        };
        assert_eq!(snapshot.workspace_token, opened.workspace_token);
        assert_eq!(snapshot.root, opened.root);
        assert!(snapshot.files.is_empty());
        assert!(snapshot.directories.is_empty());
        assert_eq!(filesystem.remove_dir_all_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert!(!canonical_target.exists());
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_document)
                .unwrap(),
            None,
        );
        assert_eq!(
            state
                .file_authorization()
                .internal_asset_grant_snapshot_for_test(&canonical_target)
                .unwrap(),
            None,
        );
        assert!(state
            .file_authorization()
            .preview_lease_snapshot()
            .unwrap()
            .is_empty());
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());
        assert_eq!(
            lock_events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
            ],
        );
    }

    #[test]
    fn delete_observation_failure_is_indeterminate_and_suspends_authority() {
        struct DeleteObservationFailurePort {
            remove_file_calls: Cell<usize>,
            observe_calls: Cell<usize>,
        }

        impl FileSystemPort for DeleteObservationFailurePort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("delete observation test must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("delete observation test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("delete observation test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("delete observation test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                self.remove_file_calls.set(self.remove_file_calls.get() + 1);
                Err(std::io::Error::other("injected delete failure"))
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("delete observation test must not delete directories")
            }

            fn observe(
                &self,
                _path: &Path,
                _expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                self.observe_calls.set(self.observe_calls.get() + 1);
                Err(std::io::Error::other("injected observation failure"))
            }
        }

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.html");
        fs::write(&document, "<h1>draft</h1>").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &document).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(&state, &document, "preview")
            .unwrap();
        let canonical_document = document.canonicalize().unwrap();
        let filesystem = DeleteObservationFailurePort {
            remove_file_calls: Cell::new(0),
            observe_calls: Cell::new(0),
        };

        let outcome = delete_workspace_entry_with_ports_inner(
            &state,
            &opened.workspace_token,
            &canonical_document,
            &filesystem,
            |_source| Err("snapshot must not run for an indeterminate delete".to_string()),
        )
        .expect("post-attempt delete errors must remain mutation outcomes");

        let value = serde_json::to_value(outcome).unwrap();
        assert_eq!(value["status"], "indeterminate");
        assert_eq!(value["operation"], "delete");
        assert!(value["recovery_message"]
            .as_str()
            .unwrap()
            .contains("injected observation failure"));
        assert_eq!(filesystem.remove_file_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert!(canonical_document.exists());
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_document)
                .unwrap()
                .map(|(status, _)| status),
            Some(GrantStatus::Suspended)
        );
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn injected_pre_call_write_failure_changes_neither_bytes_nor_grants() {
        use serde_json::json;

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.html");
        fs::write(&document, "<h1>before</h1>").unwrap();

        let state = AppState::default();
        open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &document).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(
            &state,
            &document,
            "<h1>preview</h1>",
        )
        .unwrap();

        let canonical_document = document.canonicalize().unwrap();
        let authorization_before = state.file_authorization().state_fingerprint_for_test();
        let sites_before = state.html_preview_server.site_documents().unwrap();
        let filesystem = ScriptedFileSystemPort::default();

        let outcome = write_file_with_preflight_and_ports_inner(
            &state,
            &canonical_document,
            "<h1>after</h1>",
            |path| {
                assert_eq!(path, canonical_document);
                Err("injected pre-call write failure".to_string())
            },
            &filesystem,
        )
        .expect("pre-call failures must be returned as mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-not-committed",
                "message": "injected pre-call write failure",
            })
        );
        assert_eq!(filesystem.write_calls.get(), 0);
        assert_eq!(
            fs::read_to_string(&canonical_document).unwrap(),
            "<h1>before</h1>"
        );
        assert_eq!(
            state.file_authorization().state_fingerprint_for_test(),
            authorization_before
        );
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            sites_before
        );
    }

    #[test]
    fn partial_existing_write_is_indeterminate() {
        use serde_json::json;

        struct PartialWriteFileSystemPort {
            write_calls: Cell<usize>,
            observe_calls: Cell<usize>,
            observed_path: RefCell<Option<PathBuf>>,
            observed_expected_bytes: RefCell<Option<Vec<u8>>>,
        }

        impl FileSystemPort for PartialWriteFileSystemPort {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                self.write_calls.set(self.write_calls.get() + 1);
                fs::write(path, &bytes[..4])?;
                Err(std::io::Error::other("injected partial write failure"))
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                self.observe_calls.set(self.observe_calls.get() + 1);
                *self.observed_path.borrow_mut() = Some(path.to_path_buf());
                *self.observed_expected_bytes.borrow_mut() = expected_bytes.map(<[u8]>::to_vec);
                observe_path(path, expected_bytes)
            }
        }

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.html");
        fs::write(&document, "<h1>before</h1>").unwrap();

        let state = AppState::default();
        open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &document).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(
            &state,
            &document,
            "<h1>preview</h1>",
        )
        .unwrap();

        let canonical_document = document.canonicalize().unwrap();
        assert!(ensure_authorized_write_file_inner(&state, &canonical_document).is_ok());
        assert!(ensure_authorized_directory_inner(&state, workspace.path()).is_ok());
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .contains(&canonical_document));
        assert_eq!(
            state
                .file_authorization()
                .preview_lease_snapshot()
                .unwrap()
                .len(),
            1
        );
        let filesystem = PartialWriteFileSystemPort {
            write_calls: Cell::new(0),
            observe_calls: Cell::new(0),
            observed_path: RefCell::new(None),
            observed_expected_bytes: RefCell::new(None),
        };

        let (outcome, lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
            write_file_with_ports_inner(&state, &canonical_document, "<h1>after</h1>", &filesystem)
        });
        let outcome =
            outcome.expect("post-call write errors must be returned as mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "write",
                "paths": [canonical_document.to_string_lossy()],
                "recovery_message": "Write may have partially changed the file after an error: Failed to write file: injected partial write failure. Reopen and inspect it before retrying.",
            })
        );
        assert_eq!(filesystem.write_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert_eq!(
            filesystem.observed_path.borrow().as_deref(),
            Some(canonical_document.as_path())
        );
        assert_eq!(
            filesystem.observed_expected_bytes.borrow().as_deref(),
            Some(b"<h1>after</h1>".as_slice())
        );
        assert_eq!(fs::read(&canonical_document).unwrap(), b"<h1>");
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_document)
                .unwrap(),
            Some((GrantStatus::Suspended, 1))
        );
        assert!(ensure_authorized_write_file_inner(&state, &canonical_document).is_err());
        assert!(ensure_authorized_directory_inner(&state, workspace.path()).is_ok());
        assert!(state
            .file_authorization()
            .preview_lease_snapshot()
            .unwrap()
            .is_empty());
        assert!(!state
            .html_preview_server
            .site_documents()
            .unwrap()
            .contains(&canonical_document));
        assert_eq!(
            lock_events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
            ]
        );
    }

    #[test]
    fn write_error_after_full_commit_is_confirmed_committed() {
        use serde_json::json;

        struct FullWriteThenErrorPort {
            write_calls: Cell<usize>,
            observe_calls: Cell<usize>,
        }

        impl FileSystemPort for FullWriteThenErrorPort {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                self.write_calls.set(self.write_calls.get() + 1);
                fs::write(path, bytes)?;
                Err(std::io::Error::other("injected post-commit write failure"))
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("write test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("write test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("write test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("write test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("write test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                crate::path_auth::lock_order_test_probe::assert_no_locks_held();
                self.observe_calls.set(self.observe_calls.get() + 1);
                observe_path(path, expected_bytes)
            }
        }

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.html");
        fs::write(&document, "<h1>before</h1>").unwrap();
        let state = AppState::default();
        open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &document).unwrap();
        crate::html_preview_server::prepare_html_preview_inner(&state, &document, "preview")
            .unwrap();
        let canonical_document = document.canonicalize().unwrap();
        let filesystem = FullWriteThenErrorPort {
            write_calls: Cell::new(0),
            observe_calls: Cell::new(0),
        };

        let (outcome, _lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
            write_file_with_ports_inner(&state, &canonical_document, "<h1>after</h1>", &filesystem)
        });
        let outcome = outcome.expect("observed complete write must be a mutation outcome");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-committed",
                "receipt": {
                    "committed": {
                        "path": canonical_document.to_string_lossy(),
                    },
                    "workspace": {
                        "status": "not-applicable",
                    },
                },
            }),
        );
        assert_eq!(filesystem.write_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert_eq!(
            fs::read_to_string(&canonical_document).unwrap(),
            "<h1>after</h1>",
        );
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&canonical_document)
                .unwrap(),
            Some((GrantStatus::Active, 1)),
        );
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .contains(&canonical_document));
    }

    #[test]
    fn authorization_unavailable_after_partial_write_stops_all_preview_sites() {
        use serde_json::json;

        struct PoisoningPartialWriteFileSystemPort<'a> {
            state: &'a AppState,
            write_calls: Cell<usize>,
            observe_calls: Cell<usize>,
        }

        impl FileSystemPort for PoisoningPartialWriteFileSystemPort<'_> {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                self.write_calls.set(self.write_calls.get() + 1);
                fs::write(path, &bytes[..4])?;
                let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = self.state.file_authorization().open_standalone_file(
                        path,
                        |_| Ok(()),
                        |_| panic!("injected post-write authorization poison"),
                    );
                }));
                assert!(poison.is_err());
                Err(std::io::Error::other("injected partial write failure"))
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial write test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                self.observe_calls.set(self.observe_calls.get() + 1);
                observe_path(path, expected_bytes)
            }
        }

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.html");
        let unrelated = workspace.path().join("unrelated.html");
        fs::write(&document, "<h1>before</h1>").unwrap();
        fs::write(&unrelated, "<h1>unrelated</h1>").unwrap();

        let state = AppState::default();
        open_directory_inner(&state, workspace.path()).unwrap();
        for path in [&document, &unrelated] {
            open_workspace_file_inner(&state, path).unwrap();
            crate::html_preview_server::prepare_html_preview_inner(
                &state,
                path,
                "<h1>preview</h1>",
            )
            .unwrap();
        }

        let canonical_document = document.canonicalize().unwrap();
        let canonical_unrelated = unrelated.canonicalize().unwrap();
        assert_eq!(
            state.html_preview_server.site_documents().unwrap(),
            HashSet::from([canonical_document.clone(), canonical_unrelated])
        );
        let filesystem = PoisoningPartialWriteFileSystemPort {
            state: &state,
            write_calls: Cell::new(0),
            observe_calls: Cell::new(0),
        };

        let (outcome, lock_events) = crate::path_auth::lock_order_test_probe::trace(|| {
            write_file_with_ports_inner(&state, &canonical_document, "<h1>after</h1>", &filesystem)
        });
        let outcome = outcome.expect("post-call cleanup failures must remain mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "write",
                "paths": [canonical_document.to_string_lossy()],
                "recovery_message": "Write may have partially changed the file after an error: Failed to write file: injected partial write failure. Reopen and inspect it before retrying. Authorization suspension also failed: Authorization state is poisoned. All HTML preview sites were stopped.",
            })
        );
        assert_eq!(filesystem.write_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert_eq!(fs::read(&canonical_document).unwrap(), b"<h1>");
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());
        assert!(ensure_authorized_write_file_inner(&state, &canonical_document).is_err());
        assert_eq!(
            lock_events,
            [
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::AuthorizationReleased,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesAcquired,
                crate::path_auth::lock_order_test_probe::LockEvent::HtmlSitesReleased,
            ]
        );
    }

    #[test]
    fn partial_save_as_is_indeterminate_and_ungranted() {
        use serde_json::json;

        struct PartialSaveAsFileSystemPort {
            write_calls: Cell<usize>,
            observe_calls: Cell<usize>,
            observed_path: RefCell<Option<PathBuf>>,
            observed_expected_bytes: RefCell<Option<Vec<u8>>>,
        }

        impl FileSystemPort for PartialSaveAsFileSystemPort {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                self.write_calls.set(self.write_calls.get() + 1);
                fs::write(path, &bytes[..4])?;
                Err(std::io::Error::other("injected partial save-as failure"))
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial save-as test must not create workspace files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial save-as test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("partial save-as test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial save-as test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial save-as test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                self.observe_calls.set(self.observe_calls.get() + 1);
                *self.observed_path.borrow_mut() = Some(path.to_path_buf());
                *self.observed_expected_bytes.borrow_mut() = expected_bytes.map(<[u8]>::to_vec);
                observe_path(path, expected_bytes)
            }
        }

        let directory = tempdir().unwrap();
        let destination = directory.path().join("saved.html");
        let asset = directory.path().join("asset.png");
        fs::write(&asset, b"png").unwrap();
        let canonical_directory = directory.path().canonicalize().unwrap();
        let normalized_destination = canonical_directory.join("saved.html");
        let canonical_asset = asset.canonicalize().unwrap();
        let content = "<h1>saved</h1>";

        let state = AppState::default();
        let filesystem = PartialSaveAsFileSystemPort {
            write_calls: Cell::new(0),
            observe_calls: Cell::new(0),
            observed_path: RefCell::new(None),
            observed_expected_bytes: RefCell::new(None),
        };

        let outcome = save_as_with_ports_inner(&state, &destination, content, &filesystem)
            .expect("post-call save-as errors must be returned as mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "write",
                "paths": [normalized_destination.to_string_lossy()],
                "recovery_message": "Write may have partially changed the file after an error: Failed to write file: injected partial save-as failure. Reopen and inspect it before retrying.",
            })
        );
        assert_eq!(filesystem.write_calls.get(), 1);
        assert_eq!(filesystem.observe_calls.get(), 1);
        assert_eq!(
            filesystem.observed_path.borrow().as_deref(),
            Some(normalized_destination.as_path())
        );
        assert_eq!(
            filesystem.observed_expected_bytes.borrow().as_deref(),
            Some(content.as_bytes())
        );
        assert_eq!(fs::read(&normalized_destination).unwrap(), b"<h1>");
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&normalized_destination)
                .unwrap(),
            None
        );
        assert!(ensure_authorized_write_file_inner(&state, &normalized_destination).is_err());
        assert!(!is_authorized_image_path(&state, &canonical_asset).unwrap());
        assert!(state
            .file_authorization()
            .preview_lease_snapshot()
            .unwrap()
            .is_empty());
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn save_as_grant_publication_failure_is_indeterminate_and_ungranted() {
        use serde_json::json;

        let directory = tempdir().unwrap();
        let destination = directory.path().join("saved.html");
        let normalized_destination = directory.path().canonicalize().unwrap().join("saved.html");
        let content = "<h1>saved</h1>";
        let state = AppState::default();
        state
            .file_authorization()
            .fail_next_save_publish("injected save grant publication failure")
            .unwrap();

        let outcome =
            save_as_with_ports_inner(&state, &destination, content, &SystemFileSystemPort)
                .expect("post-write grant publication failures must be mutation outcomes");

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "indeterminate",
                "operation": "write",
                "paths": [normalized_destination.to_string_lossy()],
                "recovery_message": "File contents were written, but save-as authorization could not be committed: injected save grant publication failure. Reopen and inspect the file before retrying.",
            }),
        );
        assert_eq!(
            fs::read_to_string(&normalized_destination).unwrap(),
            content
        );
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&normalized_destination)
                .unwrap(),
            None,
        );
        assert!(ensure_authorized_write_file_inner(&state, &normalized_destination).is_err());
        assert!(state
            .file_authorization()
            .preview_lease_snapshot()
            .unwrap()
            .is_empty());
        assert!(state
            .html_preview_server
            .site_documents()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn delete_commit_survives_post_commit_snapshot_failure() {
        use serde_json::json;

        let outer = tempdir().unwrap();
        let inner = outer.path().join("inner");
        let removed = inner.join("removed");
        let old_document = removed.join("index.html");
        let old_asset = removed.join("asset.png");
        fs::create_dir_all(&removed).unwrap();
        fs::write(&old_document, "<h1>before</h1>").unwrap();
        fs::write(&old_asset, b"png").unwrap();

        let state = AppState::default();
        let outer_opened = open_directory_inner(&state, outer.path()).unwrap();
        let opened = open_directory_inner(&state, &inner).unwrap();
        open_standalone_file_with_ports_inner(
            &state,
            &old_document,
            |file| open_authorized_file_response(file.to_path_buf()),
            |_| Ok(()),
        )
        .unwrap();
        crate::html_preview_server::prepare_html_preview_inner(
            &state,
            &old_document,
            "<h1>preview</h1>",
        )
        .unwrap();

        let canonical_removed = removed.canonicalize().unwrap();
        let canonical_old_document = old_document.canonicalize().unwrap();
        let snapshot_calls = std::cell::Cell::new(0);

        let outcome = delete_workspace_entry_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            &canonical_removed,
            |_source: WorkspaceSnapshotSource<'_>| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                assert!(!canonical_removed.exists());
                assert!(!state
                    .html_preview_server
                    .site_documents()
                    .unwrap()
                    .contains(&canonical_old_document));
                assert!(state
                    .file_authorization()
                    .preview_lease_snapshot()
                    .unwrap()
                    .is_empty());
                resolve_authorized_workspace_root_for_token_inner(
                    &state,
                    &opened.workspace_token,
                    &inner,
                )
                .expect("selected workspace provenance must remain active");
                resolve_authorized_workspace_root_for_token_inner(
                    &state,
                    &outer_opened.workspace_token,
                    outer.path(),
                )
                .expect("ancestor workspace provenance must remain active");
                Err("injected post-commit snapshot failure".to_string())
            },
        )
        .expect("committed delete must not become command failure");

        assert_eq!(snapshot_calls.get(), 1);
        assert!(!canonical_removed.exists());
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({
                "status": "confirmed-committed",
                "receipt": {
                    "committed": {
                        "deleted_path": canonical_removed.to_string_lossy(),
                    },
                    "workspace": {
                        "status": "stale",
                        "workspace_token": opened.workspace_token,
                        "repair_reason": "injected post-commit snapshot failure",
                    },
                },
            }),
        );

        let refreshed = refresh_directory_inner(&state, &opened.workspace_token, &inner).unwrap();
        assert!(!refreshed
            .directories
            .iter()
            .any(|entry| entry.relative_path == "removed"));
        refresh_directory_inner(&state, &outer_opened.workspace_token, outer.path()).unwrap();

        fs::create_dir_all(&removed).unwrap();
        fs::write(&old_document, "<h1>recreated</h1>").unwrap();
        assert!(ensure_authorized_existing_file_inner(&state, &old_document).is_ok());
        assert!(ensure_authorized_write_file_inner(&state, &old_document).is_err());
    }

    #[test]
    fn renaming_directory_preserves_write_authorization_for_open_descendants() {
        let dir = tempdir().unwrap();
        let notes = dir.path().join("notes");
        let doc = notes.join("doc.md");
        fs::create_dir(&notes).unwrap();
        fs::write(&doc, "# before").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, dir.path()).unwrap();
        open_workspace_file_inner(&state, &doc).unwrap();

        let renamed = committed_rename_response(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &notes,
            "archive",
        ));
        let renamed_doc = Path::new(&renamed.new_path).join("doc.md");

        write_file_inner(&state, &renamed_doc, "# after").unwrap();
        assert_eq!(fs::read_to_string(renamed_doc).unwrap(), "# after");
    }

    #[test]
    fn moving_directory_preserves_write_authorization_for_open_descendants() {
        let dir = tempdir().unwrap();
        let notes = dir.path().join("notes");
        let archive = dir.path().join("archive");
        let doc = notes.join("doc.md");
        fs::create_dir(&notes).unwrap();
        fs::create_dir(&archive).unwrap();
        fs::write(&doc, "# before").unwrap();
        let canonical_archive = fs::canonicalize(&archive).unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, dir.path()).unwrap();
        open_workspace_file_inner(&state, &doc).unwrap();

        let moved = committed_rename_response(move_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &notes,
            &archive,
        ));
        let moved_doc = Path::new(&moved.new_path).join("doc.md");

        assert_eq!(Path::new(&moved.new_path), canonical_archive.join("notes"));
        assert!(!notes.exists());
        write_file_inner(&state, &moved_doc, "# after").unwrap();
        assert_eq!(fs::read_to_string(moved_doc).unwrap(), "# after");
    }

    #[test]
    fn moving_workspace_entries_rejects_outside_descendant_and_existing_destinations() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let notes = workspace.path().join("notes");
        let nested = notes.join("nested");
        let archive = workspace.path().join("archive");
        let document = notes.join("draft.md");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&archive).unwrap();
        fs::write(&document, "# draft").unwrap();
        fs::write(archive.join("draft.md"), "# existing").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();

        assert_confirmed_not_committed(move_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &document,
            outside.path(),
        ));
        assert_confirmed_not_committed(move_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &notes,
            &nested,
        ));
        assert_confirmed_not_committed(move_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &document,
            &archive,
        ));

        assert!(document.is_file());
        assert!(notes.is_dir());
        assert_eq!(
            fs::read_to_string(archive.join("draft.md")).unwrap(),
            "# existing"
        );
    }

    #[test]
    fn workspace_image_rename_preserves_its_file_kind() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("cover.png");
        fs::write(&image, [0x89, b'P', b'N', b'G']).unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, dir.path()).unwrap();

        let renamed = committed_rename_response(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &image,
            "hero.webp",
        ));

        assert!(renamed.new_path.ends_with("hero.webp"));
        assert_confirmed_not_committed(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &renamed.new_path,
            "hero.md",
        ));
    }

    #[test]
    fn workspace_html_and_media_renames_preserve_their_file_kinds() {
        let dir = tempdir().unwrap();
        let html = dir.path().join("index.html");
        let video = dir.path().join("clip.mp4");
        fs::write(&html, "<h1>Home</h1>").unwrap();
        fs::write(&video, b"video").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, dir.path()).unwrap();

        let renamed_html = committed_rename_response(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &html,
            "home.htm",
        ));
        let renamed_video = committed_rename_response(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &video,
            "movie.flv",
        ));

        assert!(renamed_html.new_path.ends_with("home.htm"));
        assert!(renamed_video.new_path.ends_with("movie.flv"));
        assert_confirmed_not_committed(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &renamed_html.new_path,
            "home.md",
        ));
        assert_confirmed_not_committed(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &renamed_video.new_path,
            "movie.mp3",
        ));
    }

    #[test]
    fn rename_preserves_kind_and_rejects_cross_kind_target() {
        let cases = [
            ("document.md", "document.mdx", "document.html"),
            ("page.html", "page.htm", "page.png"),
            ("cover.png", "cover.webp", "cover.mp4"),
            ("clip.mp4", "clip.webm", "clip.mp3"),
            ("track.mp3", "track.wav", "track.md"),
        ];

        for (source_name, same_kind_name, cross_kind_name) in cases {
            let workspace = tempdir().unwrap();
            let source = workspace.path().join(source_name);
            fs::write(&source, b"content").unwrap();
            let state = AppState::default();
            let opened = open_directory_inner(&state, workspace.path()).unwrap();

            let same_kind = committed_rename_response(rename_workspace_entry_inner(
                &state,
                &opened.workspace_token,
                &source,
                same_kind_name,
            ));
            let same_kind_path = workspace.path().join(same_kind_name);
            let cross_kind_path = workspace.path().join(cross_kind_name);

            assert_eq!(
                Path::new(&same_kind.new_path),
                same_kind_path.canonicalize().unwrap()
            );
            assert!(!source.exists());
            assert!(same_kind_path.is_file());
            assert_confirmed_not_committed(rename_workspace_entry_inner(
                &state,
                &opened.workspace_token,
                &same_kind_path,
                cross_kind_name,
            ));
            assert!(same_kind_path.is_file());
            assert!(!cross_kind_path.exists());
        }

        let workspace = tempdir().unwrap();
        let unsupported = workspace.path().join("notes.txt");
        let promoted = workspace.path().join("notes.md");
        fs::write(&unsupported, "notes").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();

        assert_confirmed_not_committed(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &unsupported,
            "notes.md",
        ));
        assert!(unsupported.is_file());
        assert!(!promoted.exists());
    }

    #[test]
    fn workspace_mutations_reject_traversal_and_outside_paths() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();

        assert_confirmed_not_committed(create_workspace_file_inner(
            &state,
            &opened.workspace_token,
            workspace.path(),
            "../escape.md",
        ));
        assert_confirmed_not_committed(create_workspace_directory_inner(
            &state,
            &opened.workspace_token,
            outside.path(),
            "notes",
        ));

        let outside_doc = outside.path().join("outside.md");
        fs::write(&outside_doc, "# outside").unwrap();
        assert_confirmed_not_committed(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &outside_doc,
            "renamed.md",
        ));
        assert_confirmed_not_committed(delete_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            outside.path(),
        ));
    }

    #[test]
    fn workspace_root_cannot_be_renamed_or_deleted() {
        let workspace = tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, &root).unwrap();

        assert_confirmed_not_committed(rename_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &root,
            "renamed",
        ));
        assert_confirmed_not_committed(delete_workspace_entry_inner(
            &state,
            &opened.workspace_token,
            &root,
        ));
        assert!(root.is_dir());
    }

    #[test]
    fn workspace_directory_listing_includes_empty_directories() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        fs::create_dir(dir.path().join("notes")).unwrap();
        fs::write(dir.path().join("notes/doc.md"), "# doc").unwrap();
        let state = AppState::default();

        let opened = open_directory_inner(&state, dir.path()).unwrap();
        let directories: Vec<_> = opened
            .directories
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect();
        assert!(directories.contains(&"empty"));
        assert!(directories.contains(&"notes"));
    }

    #[test]
    fn directory_open_deep_intent_owns_snapshot_transport_and_publication_order() {
        use std::cell::RefCell;

        let workspace = tempdir().unwrap();
        fs::write(workspace.path().join("doc.md"), "# doc").unwrap();
        let canonical_root = workspace.path().canonicalize().unwrap();
        let state = AppState::default();
        let events = RefCell::new(Vec::new());

        let response = open_directory_with_ports_inner(
            &state,
            workspace.path(),
            |source| {
                assert!(matches!(source, WorkspaceSnapshotSource::Candidate(_)));
                events.borrow_mut().push("candidate");
                let snapshot = capture_workspace_snapshot(source)?;
                events.borrow_mut().push("snapshot");
                Ok(snapshot)
            },
            |root: &Path| {
                assert_eq!(*events.borrow(), ["candidate", "snapshot"]);
                assert_eq!(root, canonical_root);
                events.borrow_mut().push("transport");
                Ok(())
            },
        )
        .unwrap();
        events.borrow_mut().push("response");

        assert_eq!(
            *events.borrow(),
            ["candidate", "snapshot", "transport", "response"]
        );
        assert_eq!(response.root, canonical_root.to_string_lossy());
        assert_eq!(response.files.len(), 1);
        assert!(ensure_authorized_directory_inner(&state, &canonical_root).is_ok());
    }

    #[test]
    fn authorized_directory_refresh_is_allowed() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("doc.md"), "# doc").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, dir.path()).unwrap();
        assert_eq!(opened.files.len(), 1);

        let refreshed =
            refresh_directory_inner(&state, &opened.workspace_token, &opened.root).unwrap();
        assert_eq!(refreshed.root, opened.root);
        assert_eq!(refreshed.files.len(), 1);
        assert_eq!(refreshed.files[0].relative_path, "doc.md");
    }

    #[test]
    fn nested_workspace_refresh_keeps_the_exact_opened_root() {
        let outer = tempdir().unwrap();
        let inner = outer.path().join("inner");
        fs::create_dir(&inner).unwrap();
        fs::write(outer.path().join("outer.md"), "# outer").unwrap();
        fs::write(inner.join("inner.md"), "# inner").unwrap();
        let state = AppState::default();

        open_directory_inner(&state, outer.path()).unwrap();
        let opened_inner = open_directory_inner(&state, &inner).unwrap();
        let refreshed_inner =
            refresh_directory_inner(&state, &opened_inner.workspace_token, &opened_inner.root)
                .unwrap();

        assert_eq!(refreshed_inner.root, opened_inner.root);
        assert_eq!(refreshed_inner.files.len(), 1);
        assert_eq!(refreshed_inner.files[0].relative_path, "inner.md");
    }

    #[test]
    fn snapshot_api_rejects_raw_command_paths_by_construction() {
        struct CandidatePathIsOpaque;
        struct SnapshotSourcePartsAreOpaque;

        trait CandidateHasNoRawRootAccessor {
            fn root(&self) -> CandidatePathIsOpaque;
        }

        trait SnapshotSourceHasNoRawPartsAccessor {
            fn into_parts(self) -> SnapshotSourcePartsAreOpaque;
        }

        impl CandidateHasNoRawRootAccessor for WorkspaceCandidate {
            fn root(&self) -> CandidatePathIsOpaque {
                CandidatePathIsOpaque
            }
        }

        impl SnapshotSourceHasNoRawPartsAccessor for WorkspaceSnapshotSource<'_> {
            fn into_parts(self) -> SnapshotSourcePartsAreOpaque {
                SnapshotSourcePartsAreOpaque
            }
        }

        // Inherent methods outrank trait methods, so this compiles only when
        // command-facing opaque values have no raw path accessors.
        let assert_candidate_is_opaque = |candidate: &WorkspaceCandidate| {
            let _: CandidatePathIsOpaque = candidate.root();
        };
        let assert_source_is_opaque = |source: WorkspaceSnapshotSource<'_>| {
            let _: SnapshotSourcePartsAreOpaque = source.into_parts();
        };
        let _ = assert_candidate_is_opaque;
        let _ = assert_source_is_opaque;

        let _: for<'a> fn(
            WorkspaceSnapshotSource<'a>,
        ) -> Result<CapturedWorkspaceSnapshot, String> = capture_workspace_snapshot;

        let workspace = tempdir().unwrap();
        fs::write(workspace.path().join("document.md"), "# document").unwrap();
        let canonical_root = workspace.path().canonicalize().unwrap();
        let state = AppState::default();

        let response = open_directory_with_ports_inner(
            &state,
            workspace.path(),
            capture_workspace_snapshot,
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(response.root, canonical_root.to_string_lossy());
        assert_eq!(response.files.len(), 1);
    }

    #[test]
    fn refresh_and_committed_mutations_each_use_one_snapshot_operation() {
        use std::cell::Cell;

        let workspace = tempdir().unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let root = PathBuf::from(&opened.root);

        let refresh_calls = Cell::new(0);
        refresh_directory_with_snapshot_inner(&state, &opened.workspace_token, &root, |source| {
            assert!(matches!(source, WorkspaceSnapshotSource::Authorized(_)));
            refresh_calls.set(refresh_calls.get() + 1);
            crate::workspace_snapshot::capture_workspace_snapshot(source)
        })
        .unwrap();
        assert_eq!(refresh_calls.get(), 1);

        let notes = root.join("notes");
        let create_calls = Cell::new(0);
        let created = create_workspace_directory_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            &root,
            "notes",
            |source| {
                assert!(matches!(source, WorkspaceSnapshotSource::Authorized(_)));
                assert!(notes.is_dir());
                create_calls.set(create_calls.get() + 1);
                crate::workspace_snapshot::capture_workspace_snapshot(source)
            },
        )
        .unwrap();
        assert_eq!(create_calls.get(), 1);
        let receipt = match created {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        assert_eq!(Path::new(&receipt.committed.path), notes);
        match receipt.workspace {
            SnapshotReceipt::Fresh { snapshot } => assert!(snapshot
                .directories
                .iter()
                .any(|entry| entry.relative_path == "notes")),
            _ => panic!("expected fresh workspace receipt"),
        }

        let archive = root.join("archive");
        let rename_calls = Cell::new(0);
        let renamed = rename_workspace_entry_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            &notes,
            "archive",
            |source| {
                assert!(matches!(source, WorkspaceSnapshotSource::Authorized(_)));
                assert!(!notes.exists());
                assert!(archive.is_dir());
                rename_calls.set(rename_calls.get() + 1);
                crate::workspace_snapshot::capture_workspace_snapshot(source)
            },
        )
        .unwrap();
        assert_eq!(rename_calls.get(), 1);
        let receipt = match renamed {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        match receipt.workspace {
            SnapshotReceipt::Fresh { snapshot } => assert!(snapshot
                .directories
                .iter()
                .any(|entry| entry.relative_path == "archive")),
            _ => panic!("expected fresh workspace receipt"),
        }

        let delete_calls = Cell::new(0);
        let deleted = delete_workspace_entry_with_snapshot_inner(
            &state,
            &opened.workspace_token,
            &archive,
            |source| {
                assert!(matches!(source, WorkspaceSnapshotSource::Authorized(_)));
                assert!(!archive.exists());
                delete_calls.set(delete_calls.get() + 1);
                crate::workspace_snapshot::capture_workspace_snapshot(source)
            },
        )
        .unwrap();
        assert_eq!(delete_calls.get(), 1);
        let receipt = match deleted {
            MutationOutcome::ConfirmedCommitted { receipt } => receipt,
            _ => panic!("expected confirmed committed outcome"),
        };
        match receipt.workspace {
            SnapshotReceipt::Fresh { snapshot } => assert!(snapshot.directories.is_empty()),
            _ => panic!("expected fresh workspace receipt"),
        }
    }

    #[test]
    fn rename_error_reconciliation_preserves_prior_suspension_and_old_only_previews() {
        #[derive(Clone, Copy, Debug)]
        enum Layout {
            OldOnly,
            NewOnly,
        }

        struct PartialWriteThenErrorPort;

        impl FileSystemPort for PartialWriteThenErrorPort {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                fs::write(path, &bytes[..1])?;
                Err(std::io::Error::other("injected partial write"))
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial-write setup must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial-write setup must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("partial-write setup must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial-write setup must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("partial-write setup must not delete directories")
            }
        }

        struct RenameThenErrorPort {
            layout: Layout,
        }

        impl FileSystemPort for RenameThenErrorPort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("rename test must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename test must not create directories")
            }

            fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                if matches!(self.layout, Layout::NewOnly) {
                    fs::rename(from, to)?;
                }
                Err(std::io::Error::other("injected rename failure"))
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename test must not delete directories")
            }
        }

        for layout in [Layout::OldOnly, Layout::NewOnly] {
            let workspace = tempdir().unwrap();
            let source = workspace.path().join("drafts");
            let target = workspace.path().join("archive");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&target).unwrap();
            let source_document = source.join("index.html");
            let stale_target_document = target.join("stale.html");
            fs::write(&source_document, "source").unwrap();
            fs::write(&stale_target_document, "stale").unwrap();

            let state = AppState::default();
            let opened = open_directory_inner(&state, workspace.path()).unwrap();
            for document in [&source_document, &stale_target_document] {
                open_standalone_file_with_ports_inner(
                    &state,
                    document,
                    |file| open_authorized_file_response(file.to_path_buf()),
                    |_| Ok(()),
                )
                .unwrap();
                crate::html_preview_server::prepare_html_preview_inner(&state, document, "preview")
                    .unwrap();
            }

            let canonical_source = source.canonicalize().unwrap();
            let canonical_target = target.canonicalize().unwrap();
            let canonical_source_document = source_document.canonicalize().unwrap();
            let canonical_stale_target_document = stale_target_document.canonicalize().unwrap();
            let partial = write_file_with_ports_inner(
                &state,
                &canonical_stale_target_document,
                "changed",
                &PartialWriteThenErrorPort,
            )
            .unwrap();
            assert!(matches!(partial, MutationOutcome::Indeterminate { .. }));
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(&canonical_stale_target_document)
                    .unwrap()
                    .map(|(status, _)| status),
                Some(GrantStatus::Suspended),
            );

            let sites_before = state.html_preview_server.site_documents().unwrap();
            let leases_before = state.file_authorization().preview_lease_snapshot().unwrap();
            assert_eq!(
                sites_before,
                HashSet::from([canonical_source_document.clone()]),
                "{layout:?}",
            );
            assert_eq!(leases_before.len(), 1, "{layout:?}");

            fs::remove_dir_all(&canonical_target).unwrap();
            let outcome = rename_workspace_entry_with_ports_inner(
                &state,
                &opened.workspace_token,
                &canonical_source,
                "archive",
                &RenameThenErrorPort { layout },
                crate::workspace_snapshot::capture_workspace_snapshot,
            )
            .unwrap();

            match layout {
                Layout::OldOnly => {
                    assert!(matches!(
                        outcome,
                        MutationOutcome::ConfirmedNotCommitted { .. }
                    ));
                    assert_eq!(
                        state.html_preview_server.site_documents().unwrap(),
                        sites_before,
                    );
                    assert_eq!(
                        state.file_authorization().preview_lease_snapshot().unwrap(),
                        leases_before,
                    );
                    assert_eq!(
                        state
                            .file_authorization()
                            .exact_write_grant_snapshot_for_test(&canonical_source_document)
                            .unwrap()
                            .map(|(status, _)| status),
                        Some(GrantStatus::Active),
                    );
                }
                Layout::NewOnly => {
                    assert!(matches!(
                        outcome,
                        MutationOutcome::ConfirmedCommitted { .. }
                    ));
                    let relocated_source_document = canonical_target.join("index.html");
                    assert_eq!(
                        state
                            .file_authorization()
                            .exact_write_grant_snapshot_for_test(&relocated_source_document)
                            .unwrap()
                            .map(|(status, _)| status),
                        Some(GrantStatus::Active),
                    );
                    assert!(state
                        .html_preview_server
                        .site_documents()
                        .unwrap()
                        .is_empty());
                }
            }
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(&canonical_stale_target_document)
                    .unwrap()
                    .map(|(status, _)| status),
                Some(GrantStatus::Suspended),
                "{layout:?}",
            );
        }
    }

    #[test]
    fn rename_new_only_requires_matching_kind_and_canonical_destination() {
        #[derive(Clone, Copy, Debug)]
        enum InvalidEvidence {
            WrongKind,
            WrongCanonical,
        }

        struct InvalidNewOnlyEvidencePort {
            source: PathBuf,
            target: PathBuf,
            unrelated: PathBuf,
            evidence: InvalidEvidence,
        }

        impl FileSystemPort for InvalidNewOnlyEvidencePort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("rename evidence test must not write files")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename evidence test must not create files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename evidence test must not create directories")
            }

            fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                fs::rename(from, to)?;
                Err(std::io::Error::other("injected post-rename failure"))
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename evidence test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename evidence test must not delete directories")
            }

            fn observe(
                &self,
                path: &Path,
                expected_bytes: Option<&[u8]>,
            ) -> std::io::Result<ObservedPath> {
                assert!(expected_bytes.is_none());
                if path == self.source {
                    return Ok(ObservedPath::Missing);
                }
                assert_eq!(path, self.target);
                Ok(ObservedPath::Present {
                    canonical_path: Some(match self.evidence {
                        InvalidEvidence::WrongKind => self.target.clone(),
                        InvalidEvidence::WrongCanonical => self.unrelated.clone(),
                    }),
                    kind: match self.evidence {
                        InvalidEvidence::WrongKind => ObservedPathKind::Directory,
                        InvalidEvidence::WrongCanonical => ObservedPathKind::File,
                    },
                    content: ObservedContentEvidence::NotRequested,
                })
            }
        }

        for evidence in [InvalidEvidence::WrongKind, InvalidEvidence::WrongCanonical] {
            let workspace = tempdir().unwrap();
            let source = workspace.path().join("draft.md");
            let unrelated = workspace.path().join("unrelated.md");
            fs::write(&source, "draft").unwrap();
            fs::write(&unrelated, "unrelated").unwrap();

            let state = AppState::default();
            let opened = open_directory_inner(&state, workspace.path()).unwrap();
            open_workspace_file_inner(&state, &source).unwrap();
            let canonical_source = source.canonicalize().unwrap();
            let canonical_target = workspace.path().canonicalize().unwrap().join("archive.md");
            let canonical_unrelated = unrelated.canonicalize().unwrap();
            let snapshot_calls = Cell::new(0);
            let filesystem = InvalidNewOnlyEvidencePort {
                source: canonical_source.clone(),
                target: canonical_target.clone(),
                unrelated: canonical_unrelated,
                evidence,
            };

            let outcome = rename_workspace_entry_with_ports_inner(
                &state,
                &opened.workspace_token,
                &canonical_source,
                "archive.md",
                &filesystem,
                |_source| {
                    snapshot_calls.set(snapshot_calls.get() + 1);
                    Err("snapshot must not run for invalid rename evidence".to_string())
                },
            )
            .unwrap();

            assert!(
                matches!(outcome, MutationOutcome::Indeterminate { .. }),
                "{evidence:?}",
            );
            assert_eq!(snapshot_calls.get(), 0, "{evidence:?}");
            assert_eq!(
                state
                    .file_authorization()
                    .exact_write_grant_snapshot_for_test(&canonical_source)
                    .unwrap()
                    .map(|(status, _)| status),
                Some(GrantStatus::Suspended),
                "{evidence:?}",
            );
        }
    }

    #[test]
    fn system_rename_does_not_replace_a_destination_created_after_preflight() {
        struct RacingSystemRenamePort;

        impl FileSystemPort for RacingSystemRenamePort {
            fn write(&self, _path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
                unreachable!("rename race test must not write through the port")
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename race test must not create through the port")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename race test must not create directories")
            }

            fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                fs::write(to, b"racer-owned")?;
                SystemFileSystemPort.rename(from, to)
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename race test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("rename race test must not delete directories")
            }
        }

        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.md");
        fs::write(&source, b"source-owned").unwrap();
        let state = AppState::default();
        let opened = open_directory_inner(&state, workspace.path()).unwrap();
        let canonical_source = source.canonicalize().unwrap();
        let normalized_target = workspace.path().canonicalize().unwrap().join("archive.md");
        let snapshot_calls = Cell::new(0);

        let outcome = rename_workspace_entry_with_ports_inner(
            &state,
            &opened.workspace_token,
            &canonical_source,
            "archive.md",
            &RacingSystemRenamePort,
            |_source| {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Err("snapshot must not run after a lost rename race".to_string())
            },
        )
        .unwrap();

        assert!(matches!(outcome, MutationOutcome::Indeterminate { .. }));
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(fs::read(&normalized_target).unwrap(), b"racer-owned");
        assert_eq!(fs::read(&canonical_source).unwrap(), b"source-owned");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn system_rename_moves_an_entry_when_the_destination_is_absent() {
        let workspace = tempdir().unwrap();
        let source = workspace.path().join("draft.md");
        let target = workspace.path().join("archive.md");
        fs::write(&source, b"source-owned").unwrap();

        SystemFileSystemPort.rename(&source, &target).unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read(&target).unwrap(), b"source-owned");
    }

    #[cfg(windows)]
    mod windows_handle_bound_filesystem {
        use super::*;
        use std::{os::windows::fs::symlink_file, process::Command};

        fn create_junction(link: &Path, target: &Path) {
            let status = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .status()
                .unwrap();
            assert!(status.success(), "failed to create test junction");
        }

        #[test]
        fn writes_existing_and_new_regular_files() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let existing = parent.join("existing.md");
            let new = parent.join("new.md");
            fs::write(&existing, b"old-longer-content").unwrap();

            SystemFileSystemPort.write(&existing, b"updated").unwrap();
            SystemFileSystemPort.write(&new, b"created").unwrap();

            assert_eq!(fs::read(existing).unwrap(), b"updated");
            assert_eq!(fs::read(new).unwrap(), b"created");
        }

        #[test]
        fn renames_unicode_file_without_replacing_destination() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let source = parent.join("草稿.md");
            let destination = parent.join("定稿.md");
            fs::write(&source, b"source").unwrap();

            SystemFileSystemPort.rename(&source, &destination).unwrap();

            assert!(!source.exists());
            assert_eq!(fs::read(destination).unwrap(), b"source");
        }

        #[test]
        fn renames_file_to_a_single_code_unit_name() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let source = parent.join("source.md");
            let destination = parent.join("x");
            fs::write(&source, b"source").unwrap();

            SystemFileSystemPort.rename(&source, &destination).unwrap();

            assert!(!source.exists());
            assert_eq!(fs::read(destination).unwrap(), b"source");
        }

        #[test]
        fn rename_preserves_existing_destination_and_source() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let source = parent.join("source.md");
            let destination = parent.join("destination.md");
            fs::write(&source, b"source-owned").unwrap();
            fs::write(&destination, b"destination-owned").unwrap();

            let error = SystemFileSystemPort
                .rename(&source, &destination)
                .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(source).unwrap(), b"source-owned");
            assert_eq!(fs::read(destination).unwrap(), b"destination-owned");
        }

        #[test]
        fn renames_directory_without_replacing_destination() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let source = parent.join("drafts");
            let destination = parent.join("archive");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("nested.md"), b"nested").unwrap();

            SystemFileSystemPort.rename(&source, &destination).unwrap();

            assert!(!source.exists());
            assert_eq!(fs::read(destination.join("nested.md")).unwrap(), b"nested");
        }

        #[test]
        fn directory_rename_preserves_existing_destination_and_source() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let source = parent.join("source-directory");
            let destination = parent.join("destination-directory");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&destination).unwrap();
            fs::write(source.join("source.md"), b"source-owned").unwrap();
            fs::write(destination.join("destination.md"), b"destination-owned").unwrap();

            let error = SystemFileSystemPort
                .rename(&source, &destination)
                .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(source.join("source.md")).unwrap(), b"source-owned");
            assert_eq!(
                fs::read(destination.join("destination.md")).unwrap(),
                b"destination-owned"
            );
        }

        #[test]
        fn rename_does_not_replace_destination_created_after_preflight() {
            let workspace = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let source = parent.join("source.md");
            let destination = parent.join("destination.md");
            fs::write(&source, b"source-owned").unwrap();

            let error =
                windows_handle_files::rename_no_replace_with_hook(&source, &destination, || {
                    fs::write(&destination, b"racer-owned")
                })
                .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(source).unwrap(), b"source-owned");
            assert_eq!(fs::read(destination).unwrap(), b"racer-owned");
        }

        #[test]
        fn rename_rejects_source_and_destination_child_reparse_points() {
            let workspace = tempdir().unwrap();
            let outside = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let external_source = outside.path().join("external-source.md");
            let external_destination = outside.path().join("external-destination.md");
            let source_link = parent.join("source-link.md");
            let destination_link = parent.join("destination-link.md");
            fs::write(&external_source, b"external-source-owned").unwrap();
            fs::write(&external_destination, b"external-destination-owned").unwrap();
            symlink_file(&external_source, &source_link).unwrap();
            symlink_file(&external_destination, &destination_link).unwrap();

            let source_error = SystemFileSystemPort
                .rename(&source_link, &parent.join("renamed.md"))
                .unwrap_err();
            assert_eq!(source_error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                fs::read(&external_source).unwrap(),
                b"external-source-owned"
            );

            let regular_source = parent.join("regular-source.md");
            fs::write(&regular_source, b"regular-source-owned").unwrap();
            let destination_error = SystemFileSystemPort
                .rename(&regular_source, &destination_link)
                .unwrap_err();
            assert_eq!(destination_error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(fs::read(regular_source).unwrap(), b"regular-source-owned");
            assert_eq!(
                fs::read(external_destination).unwrap(),
                b"external-destination-owned"
            );
        }

        #[test]
        fn write_rejects_child_reparse_point_without_touching_external_target() {
            let workspace = tempdir().unwrap();
            let outside = tempdir().unwrap();
            let parent = workspace.path().canonicalize().unwrap();
            let external = outside.path().join("external.md");
            let child = parent.join("child.md");
            fs::write(&external, b"external-owned").unwrap();
            symlink_file(&external, &child).unwrap();

            let error = SystemFileSystemPort
                .write(&child, b"attacker-write")
                .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(fs::read(external).unwrap(), b"external-owned");
        }

        #[test]
        fn write_rejects_junction_parent_without_touching_external_target() {
            let workspace = tempdir().unwrap();
            let outside = tempdir().unwrap();
            let junction = workspace.path().join("linked");
            let external = outside.path().join("external.md");
            fs::write(&external, b"external-owned").unwrap();
            create_junction(&junction, outside.path());

            let error = SystemFileSystemPort
                .write(&junction.join("external.md"), b"attacker-write")
                .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(fs::read(external).unwrap(), b"external-owned");
        }

        #[test]
        fn rename_rejects_source_and_destination_junction_parents() {
            let workspace = tempdir().unwrap();
            let source_outside = tempdir().unwrap();
            let destination_outside = tempdir().unwrap();
            let source_junction = workspace.path().join("source-linked");
            let destination_junction = workspace.path().join("destination-linked");
            create_junction(&source_junction, source_outside.path());
            create_junction(&destination_junction, destination_outside.path());
            fs::write(source_outside.path().join("source.md"), b"source-owned").unwrap();

            let source_parent_error = SystemFileSystemPort
                .rename(
                    &source_junction.join("source.md"),
                    &workspace.path().join("target.md"),
                )
                .unwrap_err();
            assert_eq!(source_parent_error.kind(), io::ErrorKind::PermissionDenied);

            let regular_source = workspace.path().join("regular.md");
            fs::write(&regular_source, b"regular-owned").unwrap();
            let destination_parent_error = SystemFileSystemPort
                .rename(&regular_source, &destination_junction.join("target.md"))
                .unwrap_err();
            assert_eq!(
                destination_parent_error.kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(fs::read(regular_source).unwrap(), b"regular-owned");
            assert!(!destination_outside.path().join("target.md").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn system_write_rejects_existing_file_symlink_swap_without_touching_target() {
        use std::os::unix::fs::symlink;

        struct SymlinkSwapWritePort<'a> {
            outside: &'a Path,
        }

        impl FileSystemPort for SymlinkSwapWritePort<'_> {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                fs::remove_file(path)?;
                symlink(self.outside, path)?;
                SystemFileSystemPort.write(path, bytes)
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("symlink-swap test must not create workspace files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("symlink-swap test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("symlink-swap test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("symlink-swap test must not delete through the port")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("symlink-swap test must not delete directories")
            }
        }

        let workspace = tempdir().unwrap();
        let document = workspace.path().join("draft.html");
        let outside_directory = tempdir().unwrap();
        let outside = outside_directory.path().join("outside.html");
        fs::write(&document, "inside-before").unwrap();
        fs::write(&outside, "outside-before").unwrap();
        let state = AppState::default();
        open_directory_inner(&state, workspace.path()).unwrap();
        open_workspace_file_inner(&state, &document).unwrap();
        let canonical_document = document.canonicalize().unwrap();
        let canonical_outside = outside.canonicalize().unwrap();

        let outcome = write_file_with_ports_inner(
            &state,
            &canonical_document,
            "inside-after",
            &SymlinkSwapWritePort {
                outside: &canonical_outside,
            },
        )
        .unwrap();

        assert!(matches!(outcome, MutationOutcome::Indeterminate { .. }));
        assert_eq!(
            fs::read_to_string(&canonical_outside).unwrap(),
            "outside-before"
        );
        assert!(fs::symlink_metadata(&canonical_document)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn system_write_rejects_save_as_symlink_swap_without_touching_target() {
        use std::os::unix::fs::symlink;

        struct SaveAsSymlinkSwapPort<'a> {
            outside: &'a Path,
        }

        impl FileSystemPort for SaveAsSymlinkSwapPort<'_> {
            fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
                symlink(self.outside, path)?;
                SystemFileSystemPort.write(path, bytes)
            }

            fn create_new(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("save-as symlink-swap test must not create workspace files")
            }

            fn create_dir(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("save-as symlink-swap test must not create directories")
            }

            fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
                unreachable!("save-as symlink-swap test must not rename")
            }

            fn remove_file(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("save-as symlink-swap test must not delete files")
            }

            fn remove_dir_all(&self, _path: &Path) -> std::io::Result<()> {
                unreachable!("save-as symlink-swap test must not delete directories")
            }
        }

        let directory = tempdir().unwrap();
        let outside_directory = tempdir().unwrap();
        let destination = directory.path().join("saved.html");
        let outside = outside_directory.path().join("outside.html");
        fs::write(&outside, "outside-before").unwrap();
        let canonical_directory = directory.path().canonicalize().unwrap();
        let normalized_destination = canonical_directory.join("saved.html");
        let canonical_outside = outside.canonicalize().unwrap();
        let state = AppState::default();

        let outcome = save_as_with_ports_inner(
            &state,
            &destination,
            "saved content",
            &SaveAsSymlinkSwapPort {
                outside: &canonical_outside,
            },
        )
        .unwrap();

        assert!(matches!(outcome, MutationOutcome::Indeterminate { .. }));
        assert_eq!(
            fs::read_to_string(&canonical_outside).unwrap(),
            "outside-before"
        );
        assert_eq!(
            state
                .file_authorization()
                .exact_write_grant_snapshot_for_test(&normalized_destination)
                .unwrap(),
            None,
        );
        assert!(fs::symlink_metadata(&normalized_destination)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    fn session_state(app_data_dir: PathBuf) -> AppState {
        let state = AppState::default();
        state.initialize_recent_files(app_data_dir.clone()).unwrap();
        state.initialize_workspace_session(app_data_dir).unwrap();
        state
    }

    #[test]
    fn workspace_session_commands_require_the_main_window_owner() {
        assert!(validate_workspace_session_owner("main").is_ok());
        assert!(validate_workspace_session_owner("mmd-editor-popout").is_err());
        assert!(validate_workspace_session_owner("mmd-preview-popout").is_err());
    }

    #[test]
    fn workspace_session_restore_returns_none_without_a_saved_session() {
        let app_data = tempdir().unwrap();
        let state = session_state(app_data.path().to_path_buf());

        assert!(restore_workspace_session_inner(&state).unwrap().is_none());
    }

    #[test]
    fn workspace_session_restore_reopens_the_workspace_and_prepares_the_last_active_file() {
        let app_data = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let document = workspace.path().join("notes.md");
        fs::write(&document, "# Notes").unwrap();
        let original_state = session_state(app_data.path().to_path_buf());
        let opened = open_directory_inner(&original_state, workspace.path()).unwrap();
        let canonical_document = document.canonicalize().unwrap();

        persist_workspace_session_inner(
            &original_state,
            &opened.workspace_token,
            &opened.root,
            canonical_document.to_str(),
        )
        .unwrap();

        let restored_state = session_state(app_data.path().to_path_buf());
        let restored = restore_workspace_session_inner(&restored_state)
            .unwrap()
            .expect("saved workspace session is restored");
        let prepared = restored
            .active_file
            .expect("saved active file is prepared for commit");

        assert_eq!(restored.workspace.root, opened.root);
        assert_eq!(prepared.file.path, canonical_document.to_string_lossy());
        assert!(ensure_authorized_write_file_inner(&restored_state, &canonical_document).is_err());
        assert!(matches!(
            restored_state
                .recent_files()
                .unwrap()
                .commit_open(
                    &prepared.open_receipt,
                    "main",
                    restored_state.file_authorization(),
                )
                .unwrap(),
            OpenCommitResult::Committed { .. }
        ));
        assert!(ensure_authorized_write_file_inner(&restored_state, &canonical_document).is_ok());
    }

    #[test]
    fn missing_workspace_session_root_is_cleared_without_authorizing_a_workspace() {
        let app_data = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let canonical_root = workspace.path().canonicalize().unwrap();
        let state = session_state(app_data.path().to_path_buf());
        state
            .workspace_session()
            .unwrap()
            .save(&WorkspaceSessionRecord::new(
                canonical_root.to_string_lossy().to_string(),
                None,
            ))
            .unwrap();
        drop(workspace);

        assert!(restore_workspace_session_inner(&state).unwrap().is_none());
        assert!(state.workspace_session().unwrap().load().unwrap().is_none());
        assert!(ensure_authorized_directory_inner(&state, &canonical_root).is_err());
    }

    #[test]
    fn workspace_session_root_that_disappears_during_restore_is_cleared() {
        let app_data = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let canonical_root = workspace.path().canonicalize().unwrap();
        let state = session_state(app_data.path().to_path_buf());
        state
            .workspace_session()
            .unwrap()
            .save(&WorkspaceSessionRecord::new(
                canonical_root.to_string_lossy().to_string(),
                None,
            ))
            .unwrap();

        let restored = restore_workspace_session_with_ports_inner(&state, "main", |root| {
            fs::remove_dir_all(root).unwrap();
            Err("workspace disappeared during asset scope setup".to_string())
        })
        .unwrap();

        assert!(restored.is_none());
        assert!(state.workspace_session().unwrap().load().unwrap().is_none());
        assert!(ensure_authorized_directory_inner(&state, &canonical_root).is_err());
    }

    #[test]
    fn invalid_saved_active_file_is_cleared_while_its_workspace_is_restored() {
        let app_data = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let unsupported = workspace.path().join("unsupported.txt");
        let excluded_directory = workspace.path().join(".git");
        let excluded_document = excluded_directory.join("hidden.md");
        let outside_document = outside.path().join("outside.md");
        fs::create_dir(&excluded_directory).unwrap();
        fs::write(&unsupported, "not a document").unwrap();
        fs::write(&excluded_document, "# hidden").unwrap();
        fs::write(&outside_document, "# outside").unwrap();
        let canonical_root = workspace.path().canonicalize().unwrap();
        let canonical_outside = outside_document.canonicalize().unwrap();

        for active_path in [
            canonical_root.join("missing.md"),
            unsupported.canonicalize().unwrap(),
            excluded_document.canonicalize().unwrap(),
            canonical_outside.clone(),
        ] {
            let state = session_state(app_data.path().to_path_buf());
            state
                .workspace_session()
                .unwrap()
                .save(&WorkspaceSessionRecord::new(
                    canonical_root.to_string_lossy().to_string(),
                    Some(active_path.to_string_lossy().to_string()),
                ))
                .unwrap();

            let restored = restore_workspace_session_inner(&state)
                .unwrap()
                .expect("valid workspace root remains restorable");
            assert_eq!(restored.workspace.root, canonical_root.to_string_lossy());
            assert!(restored.active_file.is_none());
            assert_eq!(
                state
                    .workspace_session()
                    .unwrap()
                    .load()
                    .unwrap()
                    .expect("workspace session root remains")
                    .active_path(),
                None
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_saved_active_file_is_not_restored() {
        use std::os::unix::fs::symlink;

        let app_data = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_document = outside.path().join("outside.md");
        let linked_document = workspace.path().join("linked.md");
        fs::write(&outside_document, "# outside").unwrap();
        symlink(&outside_document, &linked_document).unwrap();
        let canonical_root = workspace.path().canonicalize().unwrap();
        let state = session_state(app_data.path().to_path_buf());
        state
            .workspace_session()
            .unwrap()
            .save(&WorkspaceSessionRecord::new(
                canonical_root.to_string_lossy().to_string(),
                Some(linked_document.to_string_lossy().to_string()),
            ))
            .unwrap();

        let restored = restore_workspace_session_inner(&state)
            .unwrap()
            .expect("workspace root remains restorable");
        assert!(restored.active_file.is_none());
        assert_eq!(
            state
                .workspace_session()
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .active_path(),
            None
        );
    }

    #[test]
    fn workspace_session_persistence_requires_the_current_workspace_and_a_supported_descendant() {
        let app_data = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let document = workspace.path().join("notes.md");
        let outside_document = outside.path().join("outside.md");
        fs::write(&document, "# Notes").unwrap();
        fs::write(&outside_document, "# Outside").unwrap();
        let state = session_state(app_data.path().to_path_buf());
        let opened = open_directory_inner(&state, workspace.path()).unwrap();

        assert!(
            persist_workspace_session_inner(&state, "workspace-999", &opened.root, None,).is_err()
        );
        assert!(persist_workspace_session_inner(
            &state,
            &opened.workspace_token,
            &opened.root,
            outside_document.canonicalize().unwrap().to_str(),
        )
        .is_err());
        assert!(state.workspace_session().unwrap().load().unwrap().is_none());
    }
}
