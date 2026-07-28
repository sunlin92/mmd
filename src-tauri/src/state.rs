use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use crate::{
    active_document_watch::ActiveDocumentWatchState, html_preview_server::HtmlPreviewServerState,
    native_menu::NativeMenuState, path_auth::FileAuthorizationSession,
    recent_files::RecentFilesState, workspace_session::WorkspaceSessionState,
};

#[derive(Default)]
pub(crate) struct AppState {
    active_document_watch: ActiveDocumentWatchState,
    file_authorization: FileAuthorizationSession,
    native_menu: Mutex<NativeMenuState>,
    recent_files: OnceLock<RecentFilesState>,
    workspace_session: OnceLock<WorkspaceSessionState>,
    pub(crate) html_preview_server: HtmlPreviewServerState,
}

impl AppState {
    pub(crate) fn active_document_watch(&self) -> &ActiveDocumentWatchState {
        &self.active_document_watch
    }

    pub(crate) fn file_authorization(&self) -> &FileAuthorizationSession {
        &self.file_authorization
    }

    pub(crate) fn native_menu_state(&self) -> NativeMenuState {
        self.native_menu
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn set_native_save_menu_enabled(&self, enabled: bool) {
        self.native_menu
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_save_enabled(enabled);
    }

    pub(crate) fn set_native_recent_files(&self, recent_files: crate::models::RecentFilesSnapshot) {
        self.native_menu
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_recent_files(recent_files);
    }

    pub(crate) fn set_native_theme_preference(
        &self,
        selected_skin: &str,
        follow_system: bool,
    ) -> Result<(), String> {
        self.native_menu
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_theme_preference(selected_skin, follow_system)
    }

    pub(crate) fn set_native_locale_preference(
        &self,
        mode: &str,
        effective_locale: &str,
    ) -> Result<(), String> {
        self.native_menu
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_locale_preference(mode, effective_locale)
    }

    pub(crate) fn initialize_recent_files(&self, app_data_dir: PathBuf) -> Result<(), String> {
        self.recent_files
            .set(RecentFilesState::new(app_data_dir))
            .map_err(|_| "Recent files state is already initialized".to_string())
    }

    pub(crate) fn recent_files(&self) -> Result<&RecentFilesState, String> {
        self.recent_files
            .get()
            .ok_or_else(|| "Recent files state is not initialized".to_string())
    }

    pub(crate) fn initialize_workspace_session(&self, app_data_dir: PathBuf) -> Result<(), String> {
        self.workspace_session
            .set(WorkspaceSessionState::new(app_data_dir))
            .map_err(|_| "Workspace session state is already initialized".to_string())
    }

    pub(crate) fn workspace_session(&self) -> Result<&WorkspaceSessionState, String> {
        self.workspace_session
            .get()
            .ok_or_else(|| "Workspace session state is not initialized".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_menu_state_starts_with_defaults_and_retains_all_projection_fields() {
        let state = AppState::default();

        assert!(!state.native_menu_state().save_enabled());
        assert_eq!(state.native_menu_state().selected_skin(), "jinxiu-zhusha");
        state.set_native_save_menu_enabled(true);
        state
            .set_native_theme_preference("songke-zhuying", true)
            .unwrap();
        state
            .set_native_locale_preference("zh-CN", "zh-CN")
            .unwrap();
        assert!(state.native_menu_state().save_enabled());
        assert_eq!(state.native_menu_state().selected_skin(), "songke-zhuying");
        assert!(state.native_menu_state().follow_system());
        assert_eq!(state.native_menu_state().locale_mode(), "zh-CN");
        assert_eq!(state.native_menu_state().effective_locale(), "zh-CN");
    }
}
