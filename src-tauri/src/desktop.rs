//! The desktop window (Tauri): one window showing the app's own local page,
//! an optional tray icon, and magnet links from the operating system.

use crate::server::{default_data_dir, start, App, Options};
use crate::torrent::magnet_from_args;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

struct Desktop {
    server: tokio::sync::Mutex<Option<App>>,
    quitting: AtomicBool,
    tray: Mutex<bool>,
}

fn add_magnet(app: &AppHandle, magnet: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let d = app.state::<Desktop>();
        if let Some(server) = d.server.lock().await.as_ref() {
            if let Err(e) = server.add_download(&json!({"magnet": magnet, "source": "Magnet bağlantısı"})) {
                eprintln!("[magnet] {e}");
            }
        }
        show_window(&app);
    });
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn ensure_tray(app: &AppHandle) {
    let d = app.state::<Desktop>();
    let mut made = d.tray.lock().unwrap();
    if *made {
        return;
    }
    let build = || -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "open", "Aç", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Çıkış", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        let mut tray = TrayIconBuilder::with_id("main").tooltip("Compass").menu(&menu).on_menu_event(|app, e| match e.id().as_ref() {
            "open" => show_window(app),
            "quit" => {
                app.state::<Desktop>().quitting.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
        Ok(())
    };
    match build() {
        Ok(()) => *made = true,
        Err(e) => eprintln!("[tepsi] {e}"),
    }
}

fn open_external(url: &str) {
    if let Err(e) = open::that_detached(url) {
        eprintln!("[aç] {url}: {e}");
    }
}

pub fn run() {
    // Some Linux graphics drivers show an empty window with WebKitGTK's DMA-BUF renderer.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let first_magnet = magnet_from_args(&std::env::args().collect::<Vec<_>>());

    let app = tauri::Builder::default()
        // One instance: a second launch (e.g. clicking a magnet in the browser) comes to this window.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(m) = magnet_from_args(&argv) {
                add_magnet(app, m);
            }
            show_window(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup(move |app| {
            let server = tauri::async_runtime::block_on(start(Options::new(default_data_dir())))?;
            let url = server.url.clone();
            app.manage(Desktop { server: tokio::sync::Mutex::new(Some(server)), quitting: AtomicBool::new(false), tray: Mutex::new(false) });

            // macOS hands magnet links over as URLs instead of arguments.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |e| {
                    let urls: Vec<String> = e.urls().iter().map(|u| u.to_string()).collect();
                    if let Some(m) = magnet_from_args(&urls) {
                        add_magnet(&handle, m);
                    }
                });
                // No protocol registration in development, so every run doesn't touch the system.
                #[cfg(all(not(debug_assertions), any(target_os = "linux", windows)))]
                if let Err(e) = app.deep_link().register("magnet") {
                    eprintln!("[magnet] kaydedilemedi: {e}");
                }
            }

            let base = url.clone();
            let page = url::Url::parse(&url)?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(page))
                .title("Compass")
                .inner_size(1180.0, 800.0)
                .min_inner_size(760.0, 560.0)
                // Outside links (e.g. an archive.org page) open in the default browser.
                .on_navigation(move |u| {
                    if u.as_str().starts_with(&base) {
                        return true;
                    }
                    if matches!(u.scheme(), "http" | "https") {
                        open_external(u.as_str());
                    }
                    false
                })
                .on_new_window(|u, _| {
                    if matches!(u.scheme(), "http" | "https") {
                        open_external(u.as_str());
                    }
                    NewWindowResponse::Deny
                })
                .build()?;
            if let Some(m) = first_magnet.clone() {
                add_magnet(app.handle(), m);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Optional: closing hides to the tray. The default (really close) stays.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let d = app.state::<Desktop>();
                if d.quitting.load(Ordering::SeqCst) {
                    return;
                }
                let to_tray = d.server.try_lock().ok().and_then(|s| s.as_ref().map(|s| s.settings().close_to_tray)).unwrap_or(false);
                if to_tray {
                    api.prevent_close();
                    ensure_tray(app);
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Compass başlatılamadı");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            // Stop downloads and qBittorrent cleanly before leaving.
            let d = app.state::<Desktop>();
            let taken = d.server.try_lock().ok().and_then(|mut s| s.take());
            if let Some(mut server) = taken {
                api.prevent_exit();
                d.quitting.store(true, Ordering::SeqCst);
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    server.close().await;
                    app.exit(0);
                });
            }
        }
    });
}

