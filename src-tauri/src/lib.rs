mod active_document_watch;
mod commands;
mod docx_preflight;
mod excalidraw_scene;
mod html_preview_server;
mod image_resolver;
mod markdown_files;
mod models;
mod native_menu;
mod path_auth;
mod recent_files;
mod state;
mod workspace_file_kind;
mod workspace_session;

pub(crate) use path_auth::workspace_snapshot;

use active_document_watch::{
    activate_active_document_watch, reconcile_active_document_watch, start_active_document_watch,
    stop_active_document_watch,
};
use commands::{
    clear_recent_files, commit_recent_open, create_workspace_directory, create_workspace_file,
    delete_workspace_entry, discard_open_receipt, get_open_commit_status, list_recent_files,
    move_workspace_entry, open_directory_dialog, open_file_dialog, open_recent_file,
    open_workspace_file, persist_workspace_session, read_file, read_workspace_image,
    refresh_directory, remove_recent_file, rename_workspace_entry, resolve_markdown_image,
    resolve_workspace_media, restore_workspace_session, save_as_dialog,
    set_native_locale_preference, set_native_save_menu_enabled, set_native_theme_preference,
    write_file,
};
use html_preview_server::prepare_html_preview;
use state::AppState;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

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
                .initialize_workspace_session(app_data_dir)
                .map_err(|error| format!("Cannot initialize workspace session: {error}"))?;
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
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_file_dialog,
            open_workspace_file,
            list_recent_files,
            open_recent_file,
            commit_recent_open,
            get_open_commit_status,
            discard_open_receipt,
            remove_recent_file,
            clear_recent_files,
            set_native_save_menu_enabled,
            set_native_theme_preference,
            set_native_locale_preference,
            read_file,
            write_file,
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
            read_workspace_image,
            resolve_workspace_media,
            prepare_html_preview,
            start_active_document_watch,
            activate_active_document_watch,
            reconcile_active_document_watch,
            stop_active_document_watch,
        ])
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
