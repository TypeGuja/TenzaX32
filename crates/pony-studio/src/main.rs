//! Pony Studio — редактор персонажей pony-engine с веб-интерфейсом.
//!
//! Окно — `tao`, интерфейс — системная веб-панель через `wry`: на Windows
//! это WebView2 (Edge), на Linux — WebKitGTK, на macOS — WKWebView.
//! Интерфейс (ui/*.html|css|js) зашит в бинарник и отдаётся через свой
//! протокол `pony://` (на Windows — `http://pony.localhost/`); там же по
//! адресу `f?p=<путь>` отдаются картинки частей с диска.
//!
//! Вся модель (персонажи, скелет, анимации, история) живёт в `App` на
//! стороне Rust; JS присылает команды через `window.ipc.postMessage`, а
//! ответ (изменившиеся разделы состояния) приходит в `window.__pony.recv`.
//!
//! Для разработки интерфейса без пересборки: `PONY_STUDIO_UI_DIR=crates/pony-studio/ui`.
//! Для автоматических скриншотов: `PONY_GUI_AUTOEXIT_MS=<мс>`.

// Релизная сборка на Windows — без чёрного окна консоли.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod anim;
mod app;
mod files;
mod scene;

use std::borrow::Cow;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::http::{header, Request, Response};
use wry::WebViewBuilder;

/// Файлы интерфейса, зашитые в бинарник.
const UI_FILES: &[(&str, &str, &str)] = &[
    ("index.html", "text/html; charset=utf-8", include_str!("../ui/index.html")),
    ("style.css", "text/css; charset=utf-8", include_str!("../ui/style.css")),
    ("util.js", "text/javascript; charset=utf-8", include_str!("../ui/util.js")),
    ("app.js", "text/javascript; charset=utf-8", include_str!("../ui/app.js")),
    ("stage.js", "text/javascript; charset=utf-8", include_str!("../ui/stage.js")),
    ("timeline.js", "text/javascript; charset=utf-8", include_str!("../ui/timeline.js")),
    ("panels.js", "text/javascript; charset=utf-8", include_str!("../ui/panels.js")),
    ("dialogs.js", "text/javascript; charset=utf-8", include_str!("../ui/dialogs.js")),
];

enum UserEvent {
    Ipc(String),
    JobDone(Result<String, String>),
}

/// Ассеты подключены относительными путями (`assets/pony/...`) — если
/// бинарник запущен не из корня workspace, ищем его рядом.
fn find_workspace_root() -> Option<std::path::PathBuf> {
    let has_assets = |dir: &std::path::Path| dir.join("assets").join("pony").is_dir();
    let cwd = std::env::current_dir().ok();
    if cwd.as_deref().is_some_and(has_assets) {
        return None;
    }
    let mut candidates: Vec<std::path::PathBuf> = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")];
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().skip(1).map(|p| p.to_path_buf()));
    }
    if let Some(cwd) = cwd {
        candidates.extend(cwd.ancestors().skip(1).map(|p| p.to_path_buf()));
    }
    candidates.into_iter().find(|dir| has_assets(dir))
}

fn respond(status: u16, mime: &str, body: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, if status == 200 { "max-age=31536000" } else { "no-store" })
        .header("Access-Control-Allow-Origin", "*")
        .body(Cow::Owned(body))
        .expect("корректный ответ")
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| scene::percent_decode(v)))
}

/// Обработчик протокола `pony://`: интерфейс и картинки частей с диска.
fn serve(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if path == "f" {
        let query = request.uri().query().unwrap_or("");
        let Some(file) = query_param(query, "p") else { return respond(400, "text/plain", b"no path".to_vec()) };
        let ext = std::path::Path::new(&file).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let asset = match ext.as_str() {
            "svg" => scene::Asset::Svg(file),
            "psd" => scene::Asset::Psd(file, query_param(query, "layer")),
            "kra" => scene::Asset::Kra(file, query_param(query, "lf")),
            _ => scene::Asset::Png(file),
        };
        return match scene::asset_bytes(&asset) {
            Ok((mime, bytes)) => respond(200, mime, bytes),
            Err(e) => respond(404, "text/plain; charset=utf-8", e.into_bytes()),
        };
    }
    if let Some(dir) = std::env::var_os("PONY_STUDIO_UI_DIR") {
        if let Ok(bytes) = std::fs::read(std::path::Path::new(&dir).join(path)) {
            let mime = UI_FILES.iter().find(|(n, _, _)| *n == path).map(|(_, m, _)| *m).unwrap_or("application/octet-stream");
            let mut r = respond(200, mime, bytes);
            r.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
            return r;
        }
    }
    match UI_FILES.iter().find(|(name, _, _)| *name == path) {
        Some((_, mime, text)) => {
            // Без кэша: после пересборки интерфейс должен обновиться.
            let mut r = respond(200, mime, text.as_bytes().to_vec());
            r.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
            r
        }
        None => respond(404, "text/plain", format!("нет файла {path}").into_bytes()),
    }
}

fn main() -> wry::Result<()> {
    if let Some(root) = find_workspace_root() {
        if std::env::set_current_dir(&root).is_ok() {
            eprintln!("pony-studio: рабочая папка -> {} (там лежат assets/)", root.display());
        }
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let mut app = app::App::new();
    let window = WindowBuilder::new()
        .with_title(app.title())
        .with_inner_size(tao::dpi::LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(900.0, 600.0))
        .build(&event_loop)
        .expect("не удалось создать окно");

    let proxy = event_loop.create_proxy();
    let job_proxy = event_loop.create_proxy();
    let builder = WebViewBuilder::new()
        .with_custom_protocol("pony".into(), |_id, request| serve(request))
        .with_ipc_handler(move |request| {
            let _ = proxy.send_event(UserEvent::Ipc(request.into_body()));
        })
        .with_background_color((30, 31, 36, 255))
        .with_devtools(cfg!(debug_assertions))
        .with_hotkeys_zoom(false)
        .with_url(if cfg!(windows) { "http://pony.localhost/" } else { "pony://localhost/" });

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let webview = builder.build(&window)?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().expect("GTK vbox окна"))?
    };

    let autoexit_at = std::env::var("PONY_GUI_AUTOEXIT_MS").ok().and_then(|s| s.parse::<u64>().ok()).map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    let mut title = app.title();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = match autoexit_at {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        };
        if autoexit_at.is_some_and(|t| std::time::Instant::now() >= t) {
            *control_flow = ControlFlow::Exit;
            return;
        }
        match event {
            Event::UserEvent(UserEvent::Ipc(text)) => {
                let reply = match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(msg) => app.handle(&msg),
                    Err(e) => serde_json::json!({ "id": null, "error": format!("плохое сообщение: {e}"), "patch": {} }),
                };
                if let Err(e) = webview.evaluate_script(&format!("window.__pony && window.__pony.recv({reply})")) {
                    eprintln!("pony-studio: evaluate_script: {e}");
                }
                for job in app.take_jobs() {
                    let proxy = job_proxy.clone();
                    std::thread::spawn(move || {
                        let _ = proxy.send_event(UserEvent::JobDone(job()));
                    });
                }
                let new_title = app.title();
                if new_title != title {
                    window.set_title(&new_title);
                    title = new_title;
                }
                if app.exit_requested {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::JobDone(result)) => {
                let msg = app.job_done(result);
                let _ = webview.evaluate_script(&format!("window.__pony && window.__pony.recv({msg})"));
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // Несохранённые правки — спрашиваем в интерфейсе (он сам
                // пришлёт "exit"), иначе закрываемся сразу.
                if app.dirty() {
                    let _ = webview.evaluate_script("window.__pony && window.__pony.confirmClose()");
                } else {
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_serves_ui_and_files_from_disk() {
        let r = serve(Request::builder().uri("pony://localhost/").body(Vec::new()).unwrap());
        assert_eq!(r.status(), 200);
        assert!(std::str::from_utf8(r.body()).unwrap().contains("<html"));
        let svg = std::env::temp_dir().join(format!("pony studio {}.svg", std::process::id()));
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let uri = format!("pony://localhost/f?p={}&v=1", scene::percent_encode(&svg.display().to_string()));
        let r = serve(Request::builder().uri(uri).body(Vec::new()).unwrap());
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "image/svg+xml");
        let _ = std::fs::remove_file(svg);
        let r = serve(Request::builder().uri("pony://localhost/f?p=%2Fnope.png").body(Vec::new()).unwrap());
        assert_eq!(r.status(), 404);
    }
}
