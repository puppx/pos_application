// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod server;

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pos_core::*;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, RunEvent, State};
use tauri_plugin_updater::UpdaterExt;

use commands::{backup_path, dispatch, list_backups, prune_backups, App, LOCAL};

const AUTO_BACKUP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

type Cmd<'a> = State<'a, App>;

/// The desktop window's way into every POS command (see `commands.rs`).
#[tauri::command(async)]
fn call(app: Cmd<'_>, cmd: String, args: Value) -> Result<Value> {
    dispatch(&app, LOCAL, &cmd, args)
}

// ---- local network server (desktop only) ----

#[derive(Default)]
struct ServerState {
    running: Mutex<Option<server::Server>>,
    /// Why the automatic start failed, if it did.
    error: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct ServerStatus {
    running: bool,
    settings: ServerSettings,
    /// Addresses to open on other devices while running.
    urls: Vec<String>,
    error: Option<String>,
}

fn status(app: &App, srv: &ServerState) -> Result<ServerStatus> {
    let settings = app.lock().pos.server_settings()?;
    let running = srv.running.lock().unwrap_or_else(|e| e.into_inner());
    let port = running.as_ref().map(|s| s.port);
    Ok(ServerStatus {
        running: port.is_some(),
        urls: port.map(server::urls).unwrap_or_default(),
        settings,
        error: srv.error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    })
}

fn start(handle: &AppHandle, srv: &ServerState, port: u16) -> Result<()> {
    let mut running = srv.running.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = running.take() {
        old.stop();
    }
    let result = server::Server::start(handle.clone(), port);
    *srv.error.lock().unwrap_or_else(|e| e.into_inner()) = result.as_ref().err().cloned();
    *running = Some(result.map_err(PosError::Validation)?);
    Ok(())
}

#[tauri::command(async)]
fn server_status(app: Cmd<'_>, srv: State<'_, ServerState>) -> Result<ServerStatus> {
    app.with_session(LOCAL, |_, _| Ok(()))?;
    status(&app, &srv)
}

/// Save the settings, then start (or restart) or stop the server.
#[tauri::command(async)]
fn set_server(
    app: Cmd<'_>,
    srv: State<'_, ServerState>,
    handle: AppHandle,
    settings: ServerSettings,
    run: bool,
) -> Result<ServerStatus> {
    app.with_session(LOCAL, |pos, s| pos.set_server_settings(s, &settings))?;
    if run {
        start(&handle, &srv, settings.port)?;
    } else if let Some(old) = srv.running.lock().unwrap_or_else(|e| e.into_inner()).take() {
        old.stop();
    }
    status(&app, &srv)
}

// ---- full screen (desktop only) ----

/// Setting (admin, Settings > Display): "1" opens the window full screen.
const START_FULLSCREEN: &str = "start_fullscreen";

fn window_error(e: tauri::Error) -> PosError {
    PosError::Internal(format!("window: {e}"))
}

/// F11 or the top bar button. Returns whether the window is now full screen.
#[tauri::command(async)]
fn toggle_fullscreen(window: tauri::WebviewWindow) -> Result<bool> {
    let on = !window.is_fullscreen().map_err(window_error)?;
    window.set_fullscreen(on).map_err(window_error)?;
    Ok(on)
}

// ---- automatic updates (desktop only) ----

#[derive(Serialize)]
struct UpdateInfo {
    current: String,
    /// Newer version on GitHub Releases, if any.
    available: Option<String>,
    notes: Option<String>,
}

fn update_error(e: tauri_plugin_updater::Error) -> PosError {
    PosError::Internal(format!("update failed: {e}"))
}

/// Ask GitHub Releases whether a newer, correctly signed version exists.
#[tauri::command]
async fn check_update(app: Cmd<'_>, handle: AppHandle) -> Result<UpdateInfo> {
    app.with_session(LOCAL, |_, _| Ok(()))?;
    let update = handle.updater().map_err(update_error)?.check().await.map_err(update_error)?;
    Ok(UpdateInfo {
        current: handle.package_info().version.to_string(),
        available: update.as_ref().map(|u| u.version.clone()),
        notes: update.and_then(|u| u.body),
    })
}

/// Back up the database, download the update (its signature is checked
/// against the public key built into the app), install it and restart.
/// On Windows the installer closes the app and reopens it when done.
#[tauri::command]
async fn install_update(app: Cmd<'_>, handle: AppHandle) -> Result<()> {
    app.with_session(LOCAL, |pos, s| {
        s.require(Permission::ManageSettings)?;
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        pos.backup_unchecked(app.backup_dir.join(format!("pos-before-update-{ts}.db")))?;
        pos.optimize()
    })?;
    let update = handle
        .updater()
        .map_err(update_error)?
        .check()
        .await
        .map_err(update_error)?
        .ok_or_else(|| PosError::Validation("already up to date".into()))?;
    update.download_and_install(|_, _| {}, || {}).await.map_err(update_error)?;
    handle.restart()
}

/// Daily safety copy, taken at startup when the newest one is over a day old.
fn auto_backup(pos: &Pos, dir: &Path) {
    let fresh = list_backups(dir)
        .first()
        .and_then(|(t, _)| t.elapsed().ok())
        .is_some_and(|age| age < AUTO_BACKUP_EVERY);
    if !fresh && !pos.needs_setup().unwrap_or(true) {
        if let Err(e) = pos.backup_unchecked(backup_path(dir)) {
            eprintln!("auto backup failed: {e}");
        }
        prune_backups(dir);
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let backup_dir = data_dir.join("backups");
            std::fs::create_dir_all(&backup_dir)?;
            let pos = Pos::open(data_dir.join("pos.db"))?;
            auto_backup(&pos, &backup_dir);
            let server = pos.server_settings()?;
            let fullscreen = pos.setting(START_FULLSCREEN)?.as_deref() == Some("1");
            app.manage(App::new(pos, backup_dir));
            if fullscreen {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.set_fullscreen(true);
                }
            }
            app.manage(ServerState::default());
            if server.auto_start {
                let handle = app.handle().clone();
                if let Err(e) = start(&handle, &handle.state::<ServerState>(), server.port) {
                    eprintln!("local server: {e}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            call,
            server_status,
            set_server,
            toggle_fullscreen,
            check_update,
            install_update,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start POS");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(srv) = handle.try_state::<ServerState>() {
                if let Some(s) = srv.running.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    s.stop();
                }
            }
            if let Some(state) = handle.try_state::<App>() {
                let _ = state.lock().pos.optimize();
            }
        }
    });
}
