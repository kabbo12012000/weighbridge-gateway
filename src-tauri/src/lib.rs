mod config;
mod frame;
mod motion;
mod parser;
mod protocol;
mod serial;
mod state;
mod tls;
mod ws;

use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use config::{random_secret, AppConfig};
use parser::{ParseError, Parsed, Parser};
use serial::PortInfo;
use state::{Gateway, Paths, Snapshot};

type Gw<'a> = State<'a, Arc<Gateway>>;

const MINIMIZED_ARG: &str = "--minimized";

// ---- commands -----------------------------------------------------------------------

#[tauri::command]
fn get_state(gw: Gw) -> Snapshot {
    gw.snapshot()
}

#[tauri::command]
fn get_config(gw: Gw) -> AppConfig {
    gw.config()
}

#[tauri::command]
fn list_ports() -> Result<Vec<PortInfo>, String> {
    serial::list_ports()
}

#[tauri::command]
async fn save_config(gw: Gw<'_>, config: AppConfig) -> Result<AppConfig, String> {
    let mut config = config;
    config.device_id = config.device_id.trim().to_string();
    config.serial.port = config.serial.port.trim().to_string();
    config.websocket.allowed_origins = config
        .websocket
        .allowed_origins
        .iter()
        .map(|o| o.trim().trim_end_matches('/').to_string())
        .filter(|o| !o.is_empty())
        .collect();
    config.validate()?;
    let old = gw.config();
    gw.update_config(config.clone())?;

    let serial_changed =
        old.serial != config.serial || old.parser != config.parser || old.device_id != config.device_id
            || old.websocket.signing_secret != config.websocket.signing_secret;
    if serial_changed && gw.serial_running() {
        let gw = gw.inner().clone();
        tauri::async_runtime::spawn_blocking(move || gw.start_serial()).await.map_err(|e| e.to_string())?;
    }
    if (old.websocket.port_start, old.websocket.port_end) != (config.websocket.port_start, config.websocket.port_end) {
        gw.ws_restart.notify_one();
    }
    if old.websocket.token != config.websocket.token
        || old.websocket.allowed_origins != config.websocket.allowed_origins
        || old.websocket.require_auth != config.websocket.require_auth
        || old.websocket.allow_missing_origin != config.websocket.allow_missing_origin
    {
        gw.kick_clients();
    }
    Ok(config)
}

#[tauri::command]
async fn connect_serial(gw: Gw<'_>) -> Result<(), String> {
    let gw = gw.inner().clone();
    tauri::async_runtime::spawn_blocking(move || gw.start_serial()).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn disconnect_serial(gw: Gw<'_>) -> Result<(), String> {
    let gw = gw.inner().clone();
    tauri::async_runtime::spawn_blocking(move || gw.stop_serial()).await.map_err(|e| e.to_string())
}

/// `which` = "token" | "signing_secret". Returns the new value.
#[tauri::command]
async fn regenerate_secret(gw: Gw<'_>, which: String) -> Result<AppConfig, String> {
    let mut cfg = gw.config();
    match which.as_str() {
        "token" => cfg.websocket.token = random_secret(),
        "signing_secret" => cfg.websocket.signing_secret = random_secret(),
        _ => return Err("Unknown secret".into()),
    }
    save_config(gw, cfg).await
}

#[tauri::command]
fn test_parser(config: AppConfig, text: String) -> Result<Parsed, String> {
    let parser = Parser::new(&config.parser)?;
    match parser.parse(&text) {
        Ok(p) => Ok(p),
        Err(ParseError::Skipped) => Err("Raw mode: frames are not parsed".into()),
        Err(ParseError::Failed(e)) => Err(e),
    }
}

#[tauri::command]
async fn setup_tls(gw: Gw<'_>) -> Result<(), String> {
    let gw = gw.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        tls::setup(&gw.paths.tls_dir, &gw.config().device_id)?;
        let acceptor = tls::load_acceptor(&gw.paths.tls_dir)?;
        gw.set_tls(acceptor);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn remove_tls(gw: Gw<'_>) -> Result<(), String> {
    let gw = gw.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let r = tls::remove(&gw.paths.tls_dir);
        gw.set_tls(None);
        r
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn clear_monitor(gw: Gw) {
    gw.clear_monitor();
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let a = app.autolaunch();
    let result = if enabled { a.enable() } else { a.disable() };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(&path).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

// ---- app ----------------------------------------------------------------------------

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn init_logging(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("gateway")
        .filename_suffix("log")
        .max_log_files(14)
        .build(dir);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    match appender {
        Ok(appender) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(appender).try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second copy would fight over the COM port and the WebSocket port.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![MINIMIZED_ARG])))
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let log_dir = app.path().app_log_dir()?;
            init_logging(&log_dir);
            let paths = Paths {
                config_file: config_dir.join("gateway.json"),
                tls_dir: config_dir.join("tls"),
                log_dir,
            };
            let (cfg, warning) = AppConfig::load(&paths.config_file);
            tracing::info!("Weighbridge Gateway {} starting, config {}", protocol::VERSION, paths.config_file.display());
            let auto_connect = cfg.serial.auto_connect;
            let gw = Gateway::new(paths, cfg, warning);
            match tls::load_acceptor(&gw.paths.tls_dir) {
                Ok(a) => gw.set_tls(a),
                Err(e) => gw.error("tls", format!("TLS certificate could not be loaded: {e}"), false),
            }
            app.manage(gw.clone());

            // WebSocket server + heartbeat.
            tauri::async_runtime::spawn(ws::run(gw.clone()));
            let hb = gw.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let ms = hb.config().websocket.heartbeat_ms.max(1000);
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    hb.publish(&hb.heartbeat_msg());
                }
            });
            if auto_connect {
                gw.start_serial();
            }

            // Tray: the gateway keeps running when the window is closed.
            let show = MenuItem::with_id(app, "show", "Show gateway", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit gateway", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("Weighbridge Gateway")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => {
                        if let Some(gw) = app.try_state::<Arc<Gateway>>() {
                            gw.stop_serial();
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let minimized = std::env::args().any(|a| a == MINIMIZED_ARG);
            if !minimized {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Hide instead of quitting; quitting is done from the tray menu.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_config,
            list_ports,
            save_config,
            connect_serial,
            disconnect_serial,
            regenerate_secret,
            test_parser,
            setup_tls,
            remove_tls,
            clear_monitor,
            get_autostart,
            set_autostart,
            open_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Weighbridge Gateway");
}
