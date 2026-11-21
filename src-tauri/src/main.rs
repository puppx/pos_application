// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod server;

use std::sync::Mutex;
use std::time::Duration;

use pos_core::*;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, RunEvent, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_updater::UpdaterExt;

use commands::{auto_backup, dispatch, require_updates, unique_backup_path, with_device_settings, App, LOCAL};

const AUTO_BACKUP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The daily backup starts this long after the window opens, so the first
/// screens load without competing for the disk.
const AUTO_BACKUP_DELAY: Duration = Duration::from_secs(5);
/// On closing or restarting, tickets still going out to a printer get this
/// long to finish (a picture cut off half way prints the next job as junk).
const PRINTS_FINISH: Duration = Duration::from_secs(15);

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

// ---- backup folder (desktop only) ----

/// "Open backup folder": show the full backups in Explorer, to copy one to
/// a USB stick (e.g. to move to a new PC). Admins only.
#[tauri::command(async)]
fn open_backup_folder(app: Cmd<'_>) -> Result<()> {
    app.with_session(LOCAL, |_, s| {
        s.require(Permission::Backup)?;
        s.require(Permission::ManageSettings)
    })?;
    let dir = &app.backup_dir;
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    // Explorer's exit code means nothing, so only a failure to start counts.
    let mut child = cmd
        .arg(dir)
        .spawn()
        .map_err(|e| PosError::Validation(format!("Cannot open the folder here ({e}). Open it yourself: {}", dir.display())))?;
    std::thread::spawn(move || child.wait());
    Ok(())
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

// ---- start with Windows (desktop only) ----

/// The name of the entry in the user's start-up list (on Windows the value
/// under HKCU\Software\Microsoft\Windows\CurrentVersion\Run). Fixed, so
/// renaming the app never leaves an old entry behind.
const AUTOSTART_NAME: &str = "POS";

fn autostart_error(e: tauri_plugin_autostart::Error) -> PosError {
    PosError::Internal(format!("start with Windows: {e}"))
}

/// Whether the POS opens by itself when this Windows user logs in (also
/// false when it was turned off in Task Manager > Startup apps).
#[tauri::command(async)]
fn get_autostart(app: Cmd<'_>, handle: AppHandle) -> Result<bool> {
    app.with_session(LOCAL, |_, _| Ok(()))?;
    handle.autolaunch().is_enabled().map_err(autostart_error)
}

/// Settings > Devices & display > "Start with Windows" (admins only).
/// Returns whether it is now on.
#[tauri::command(async)]
fn set_autostart(app: Cmd<'_>, handle: AppHandle, enabled: bool) -> Result<bool> {
    with_device_settings(&app, LOCAL, |pos, s| {
        let auto = handle.autolaunch();
        let changed = if enabled { auto.enable() } else { auto.disable() };
        changed.map_err(autostart_error)?;
        // Windows can keep its own say: an entry for every user of the PC
        // (made by a copy of the app run as administrator) or a "Disabled"
        // in Task Manager. Say so rather than show a switch that is wrong.
        if auto.is_enabled().map_err(autostart_error)? != enabled {
            return Err(PosError::Validation(if enabled {
                "Windows has POS turned off in Task Manager > Startup apps: turn it on there".into()
            } else {
                "POS is set to start for every user of this PC: turn it off in Task Manager > Startup apps".into()
            }));
        }
        pos.audit_event(s, "device.autostart", if enabled { "on" } else { "off" })?;
        Ok(enabled)
    })
}

/// "Close POS" on the lock screen: quit the app (the exit handler stops the
/// local server and tidies the database).
#[tauri::command(async)]
fn quit_app(handle: AppHandle) {
    handle.exit(0);
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

/// Ask GitHub Releases whether a newer, correctly signed version exists
/// (admins only; for anyone else the UI shows nothing about updates).
#[tauri::command]
async fn check_update(app: Cmd<'_>, handle: AppHandle) -> Result<UpdateInfo> {
    require_updates(&app, LOCAL)?;
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
/// Admins only.
#[tauri::command]
async fn install_update(app: Cmd<'_>, handle: AppHandle) -> Result<()> {
    app.with_session(LOCAL, |pos, s| {
        s.require(Permission::Updates)?;
        pos.backup_unchecked(unique_backup_path(&app.backup_dir, "pos-before-update-").0)?;
        pos.optimize()
    })?;
    let update = handle
        .updater()
        .map_err(update_error)?
        .check()
        .await
        .map_err(update_error)?
        .ok_or_else(|| PosError::Validation("already up to date".into()))?;
    // An automatic backup still copying finishes first, and none starts
    // while the update replaces the app (it would be cut off half way).
    app.stop_auto_backups();
    let bytes = update.download(|_, _| {}, || {}).await.map_err(update_error)?;
    // On Windows `install` starts the installer and ends this process at once
    // (no exit handler runs), so tickets still printing finish first, and the
    // single-instance lock is let go: the installer reopens the app when done.
    pos_core::wait_for_prints(PRINTS_FINISH);
    #[cfg(windows)]
    tauri_plugin_single_instance::destroy(&handle);
    update.install(bytes).map_err(update_error)?;
    // Elsewhere: restart (Tauri lets go of the lock just before it starts the new copy).
    handle.restart()
}

fn main() {
    let app = tauri::Builder::default()
        // First, as its docs ask: a second copy of the POS (opened again, or by
        // "Start with Windows" while already open) ends here, before it opens
        // the database or the local server, and the open window comes forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Off until an admin turns it on: the plugin itself changes nothing.
        .plugin(tauri_plugin_autostart::Builder::new().app_name(AUTOSTART_NAME).build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let backup_dir = data_dir.join("backups");
            std::fs::create_dir_all(&backup_dir)?;
            // Prints that fail after the till was told they were sent.
            pos_core::set_print_error_log(Some(data_dir.join("print-errors.log")));
            let db = data_dir.join("pos.db");
            let mut pos = Pos::open(&db)?;
            // Orders left open with nothing on them (a table tapped, a till
            // reloaded) clutter the Orders board: cancel the old ones.
            if let Err(e) = pos.sweep_empty_orders() {
                eprintln!("empty orders: {e}");
            }
            let server = pos.server_settings()?;
            let fullscreen = pos.setting(START_FULLSCREEN)?.as_deref() == Some("1");
            app.manage(App::new(pos, backup_dir));
            if fullscreen {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.set_fullscreen(true);
                }
            }
            app.manage(ServerState::default());
            // "Start with Windows" is on: point it at this copy of the app, so
            // it still opens if the app was installed again in another folder.
            // Release builds only: a developer's debug build never takes it over.
            #[cfg(not(debug_assertions))]
            if let Some(auto) = app.try_state::<tauri_plugin_autostart::AutoLaunchManager>() {
                if auto.is_enabled().unwrap_or(false) {
                    if let Err(e) = auto.enable() {
                        eprintln!("start with Windows: {e}");
                    }
                }
            }
            // The daily backup, in the background: a big database must not
            // keep the window from opening (see `commands::auto_backup`).
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(AUTO_BACKUP_DELAY);
                auto_backup(&handle.state::<App>(), &db, AUTO_BACKUP_EVERY);
            });
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
            get_autostart,
            set_autostart,
            open_backup_folder,
            quit_app,
            check_update,
            install_update,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start POS");

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { .. } = event {
            pos_core::wait_for_prints(PRINTS_FINISH);
        }
        if let RunEvent::Exit = event {
            pos_core::wait_for_prints(PRINTS_FINISH);
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
