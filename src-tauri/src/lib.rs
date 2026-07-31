mod active_document_watch;
mod commands;
mod crash_draft_commands;
mod crash_draft_store;
mod crash_drafts;
mod document_save;
mod docx_preflight;
mod durable_write;
mod excalidraw_scene;
mod html_preview_server;
mod image_resolver;
mod markdown_files;
mod models;
mod native_menu;
#[cfg(feature = "packaged-lifecycle-e2e")]
mod packaged_lifecycle_e2e;
mod path_auth;
mod recent_files;
mod settings;
mod state;
mod workspace_file_kind;
pub mod workspace_index;
mod workspace_session;
mod workspace_trash;
mod workspace_trash_native;

pub(crate) use path_auth::workspace_snapshot;

use active_document_watch::{
    activate_active_document_watch, reconcile_active_document_watch, start_active_document_watch,
    stop_active_document_watch,
};
use commands::{
    cancel_document_overwrite_token, clear_recent_files, commit_recent_open,
    create_workspace_directory, create_workspace_file, delete_workspace_entry,
    discard_open_receipt, get_open_commit_status, get_settings, issue_document_overwrite_token,
    list_recent_files, move_workspace_entry, open_directory_dialog, open_file_dialog,
    open_recent_file, open_workspace_file, persist_workspace_session, read_file,
    read_markdown_excalidraw, read_workspace_image, refresh_directory, remove_recent_file,
    rename_workspace_entry, reset_settings, resolve_markdown_image, resolve_workspace_media,
    restore_workspace_session, retry_document_save_with_token, save_as_dialog,
    set_native_locale_preference, set_native_save_menu_enabled, set_native_theme_preference,
    update_settings, write_file,
};
use crash_draft_commands::{
    discard_crash_draft, list_crash_drafts, recover_crash_draft, reset_crash_draft_overflow_batch,
    reset_crash_drafts, write_crash_draft,
};
use html_preview_server::{
    prepare_html_preview, prepare_markdown_html_embed, release_markdown_html_embed,
    release_markdown_html_embed_window_inner,
};
#[cfg(feature = "packaged-lifecycle-e2e")]
use packaged_lifecycle_e2e::setup_packaged_lifecycle_e2e;
use state::AppState;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

macro_rules! app_invoke_handler {
    ($($extra:path),* $(,)?) => {
        tauri::generate_handler![
            open_file_dialog,
            open_workspace_file,
            list_recent_files,
            open_recent_file,
            commit_recent_open,
            get_open_commit_status,
            discard_open_receipt,
            remove_recent_file,
            clear_recent_files,
            get_settings,
            update_settings,
            reset_settings,
            set_native_save_menu_enabled,
            set_native_theme_preference,
            set_native_locale_preference,
            read_file,
            write_file,
            issue_document_overwrite_token,
            retry_document_save_with_token,
            cancel_document_overwrite_token,
            save_as_dialog,
            open_directory_dialog,
            restore_workspace_session,
            persist_workspace_session,
            refresh_directory,
            create_workspace_file,
            create_workspace_directory,
            rename_workspace_entry,
            move_workspace_entry,
            delete_workspace_entry,
            resolve_markdown_image,
            read_markdown_excalidraw,
            read_workspace_image,
            resolve_workspace_media,
            prepare_html_preview,
            prepare_markdown_html_embed,
            release_markdown_html_embed,
            start_active_document_watch,
            activate_active_document_watch,
            reconcile_active_document_watch,
            stop_active_document_watch,
            list_crash_drafts,
            write_crash_draft,
            recover_crash_draft,
            discard_crash_draft,
            reset_crash_drafts,
            reset_crash_draft_overflow_batch
            $(, $extra)*
        ]
    };
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = AppState::default();
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Cannot locate application data: {error}"))?;
            state
                .initialize_recent_files(app_data_dir.clone())
                .map_err(|error| format!("Cannot initialize recent files: {error}"))?;
            state
                .initialize_settings(app_data_dir.clone())
                .map_err(|error| format!("Cannot initialize settings: {error}"))?;
            state
                .initialize_workspace_session(app_data_dir.clone())
                .map_err(|error| format!("Cannot initialize workspace session: {error}"))?;
            state
                .initialize_crash_drafts(app_data_dir)
                .map_err(|error| format!("Cannot initialize crash recovery: {error}"))?;
            app.manage(state);
            let recent_files = app.state::<AppState>().recent_files()?.list()?;
            app.state::<AppState>()
                .set_native_recent_files(recent_files);
            app.set_menu(native_menu::build_app_menu(
                app.handle(),
                &app.state::<AppState>().native_menu_state(),
            )?)?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            let Some(route) = native_menu::route_for_menu_id(event.id().as_ref()) else {
                return;
            };
            let target = match route {
                native_menu::MenuRoute::MainFile
                | native_menu::MenuRoute::MainThemeAuthority
                | native_menu::MenuRoute::MainLocaleAuthority => "main",
            };
            if let (Some(action), Some(window)) = (
                native_menu::action_for_menu_id(event.id().as_ref()),
                app.get_webview_window(target),
            ) {
                let _ = window.emit(native_menu::NATIVE_MENU_EVENT, action);
            }
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Destroyed) {
                let state = window.app_handle().state::<AppState>();
                if window.label() == "main" {
                    state.active_document_watch().stop_all();
                }
                if let Ok(recent_files) = state.recent_files() {
                    let _ = recent_files.remove_owner(window.label());
                }
                let _ = release_markdown_html_embed_window_inner(&state, window.label());
            }
        });
    #[cfg(feature = "packaged-lifecycle-e2e")]
    let app = app.invoke_handler(app_invoke_handler!(setup_packaged_lifecycle_e2e));
    #[cfg(not(feature = "packaged-lifecycle-e2e"))]
    let app = app.invoke_handler(app_invoke_handler!());
    let app = app
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app, event| {
        if matches!(event, RunEvent::Exit) {
            let state = app.state::<AppState>();
            state.active_document_watch().stop_all();
            if let Ok(recent_files) = state.recent_files() {
                let _ = recent_files.shutdown();
            }
        }
    });
}
