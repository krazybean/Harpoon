//! A picture of a deployed application's page, for the Overview.
//!
//! An iframe cannot do this: WebKit has no `credentialless` frames, and an app may forbid framing outright
//! (`X-Frame-Options: DENY`, `frame-ancestors 'none'`). Instead a throw-away, non-persistent web view (no cookies,
//! no storage, no IPC capability) loads the page off-screen, `WKWebView.takeSnapshot` renders it, and the window is
//! closed. The image is returned to the caller and never written anywhere.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Only the loopback gateway or a published port on this Mac: this must never fetch an arbitrary address.
pub(crate) fn loopback_target(url: &str) -> Result<tauri::Url, String> {
    let u: tauri::Url = url.parse().map_err(|_| "The application address is not a valid URL.".to_string())?;
    let host = u.host_str().unwrap_or("");
    let local = host == "localhost" || host == "127.0.0.1" || host == "harpoon.localhost" || host.ends_with(".harpoon.localhost");
    if u.scheme() != "http" || !local || !u.username().is_empty() || u.password().is_some() {
        return Err("A preview is only taken of an application on this Mac (http, no credentials in the address).".into());
    }
    Ok(u)
}

/// PNG bytes of the snapshot and whether it is a single flat colour (nothing visible has been drawn yet).
#[cfg(target_os = "macos")]
fn png_of(img: &objc2_app_kit::NSImage) -> Result<(Vec<u8>, bool), String> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::NSDictionary;
    let tiff = img.TIFFRepresentation().ok_or("the snapshot has no image data")?;
    let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff).ok_or("the snapshot could not be read")?;
    let blank = {
        let (w, h, stride, spp) = (rep.pixelsWide() as usize, rep.pixelsHigh() as usize, rep.bytesPerRow() as usize, rep.samplesPerPixel() as usize);
        let base = rep.bitmapData();
        if base.is_null() || w == 0 || h == 0 || spp == 0 {
            false // cannot tell: treat as drawn rather than hide a real page
        } else {
            // SAFETY: `bitmapData` points at `h * stride` bytes owned by `rep`, which outlives this block; every offset below is inside it.
            let px = |x: usize, y: usize| unsafe { std::slice::from_raw_parts(base.add(y * stride + x * spp), spp).to_vec() };
            let first = px(0, 0);
            (0..100).all(|j| (0..160).all(|i| px(i * (w - 1) / 159, j * (h - 1) / 99) == first))
        }
    };
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
        .ok_or("the snapshot could not be encoded")?;
    Ok((png.to_vec(), blank))
}

const SETTLE_LIMIT: Duration = Duration::from_secs(12);

/// One `takeSnapshot` of the window's web view: PNG bytes and whether it is blank.
#[cfg(target_os = "macos")]
fn snapshot(win: &tauri::WebviewWindow) -> Result<(Vec<u8>, bool), String> {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSImage;
    use objc2_foundation::{NSError, NSNumber};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
    let (tx, rx) = mpsc::channel::<Result<(Vec<u8>, bool), String>>();
    win.with_webview(move |wv| {
        let mtm = MainThreadMarker::new().expect("with_webview runs on the main thread");
        // SAFETY: on macOS `inner()` is the window's WKWebView, alive for this call; the block runs on the main thread.
        unsafe {
            let wk: &WKWebView = &*(wv.inner() as *const WKWebView);
            let cfg = WKSnapshotConfiguration::new(mtm);
            cfg.setSnapshotWidth(Some(&NSNumber::new_f64(960.0)));
            let block = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
                let r = match (img.as_ref(), err.as_ref()) {
                    (Some(i), _) => png_of(i),
                    (None, Some(e)) => Err(format!("the page could not be captured: {}", e.localizedDescription())),
                    _ => Err("the page could not be captured".to_string()),
                };
                let _ = tx.send(r);
            });
            wk.takeSnapshotWithConfiguration_completionHandler(Some(&cfg), &block);
        }
    })
    .map_err(|e| format!("could not reach the capture view: {e}"))?;
    rx.recv_timeout(Duration::from_secs(15)).map_err(|_| "The capture did not complete.".to_string())?
}

/// Whether response headers carry an enforced `Content-Security-Policy` with `upgrade-insecure-requests`.
fn csp_upgrades(headers: &str) -> bool {
    headers.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("content-security-policy:") && l.contains("upgrade-insecure-requests")
    })
}

/// The response headers of `HEAD <path>` on the app, or `None` when it does not answer. Plain http to loopback only.
fn probe_headers(u: &tauri::Url) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    let (host, port) = (u.host_str()?, u.port_or_known_default()?);
    // `*.localhost` is loopback by definition (RFC 6761); the system resolver does not always say so
    let target = if host == "localhost" || host.ends_with(".localhost") { "127.0.0.1" } else { host };
    let addr = (target, port).to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let req = format!("HEAD {} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: Harpoon-preview\r\nConnection: close\r\n\r\n", u.path());
    s.write_all(req.as_bytes()).ok()?;
    // the headers may arrive in several writes; read until they end, the peer closes, or 16 KiB
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while buf.len() < 16 * 1024 && !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    (!buf.is_empty()).then(|| String::from_utf8_lossy(&buf).split("\r\n\r\n").next().unwrap_or("").to_string())
}

/// `data:image/png;base64,…` of the page at `url`, or the reason none could be made.
#[tauri::command]
pub async fn deploy_preview(app: tauri::AppHandle, url: String) -> Result<String, String> {
    let target = loopback_target(&url)?;
    // An app that sends `upgrade-insecure-requests` cannot load its own scripts and styles in WebKit over http, even on
    // loopback. Its private capture goes through the loopback relay instead (see capture_proxy.rs), which removes only
    // that directive from what this one window sees. Ordinary visitors never touch the relay.
    let Some(headers) = probe_headers(&target) else {
        return Err(format!("The application did not answer at {url}. Check that it is running, then Refresh."));
    };
    let target = if csp_upgrades(&headers) { relayed(&target)? } else { target };
    tauri::async_runtime::spawn_blocking(move || {
        // one capture window at a time: a quick switch between applications queues instead of piling windows up
        let _one = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        capture(&app, target)
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))?
}

static CAPTURE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Labels of any capture windows still open (there are none between captures).
pub fn open_capture_windows(app: &tauri::AppHandle) -> Vec<String> {
    use tauri::Manager;
    app.webview_windows().into_keys().filter(|l| l.starts_with("preview-")).collect()
}

/// The same address on the capture relay's scheme.
fn relayed(u: &tauri::Url) -> Result<tauri::Url, String> {
    let (Some(host), Some(port)) = (u.host_str(), u.port_or_known_default()) else { return Err("The application address has no host or port.".into()) };
    format!("{}://{host}:{port}{}", crate::capture_proxy::SCHEME, u.path()).parse().map_err(|_| "The application address could not be relayed.".to_string())
}

#[cfg(not(target_os = "macos"))]
fn capture(_app: &tauri::AppHandle, _target: tauri::Url) -> Result<String, String> {
    Err("Page previews are available on macOS only.".into())
}

#[cfg(target_os = "macos")]
fn capture(app: &tauri::AppHandle, target: tauri::Url) -> Result<String, String> {
    use tauri::webview::PageLoadEvent;
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    static N: AtomicU32 = AtomicU32::new(0);
    let (loaded_tx, loaded_rx) = mpsc::channel::<()>();
    let host = target.host_str().unwrap_or("").to_string();
    let win = WebviewWindowBuilder::new(app, format!("preview-{}", N.fetch_add(1, Ordering::Relaxed)), WebviewUrl::External(target))
        .incognito(true) // WKWebsiteDataStore.nonPersistent: no cookies, no storage, nothing kept
        .decorations(false)
        .focused(false)
        .resizable(false)
        .skip_taskbar(true)
        .inner_size(1280.0, 800.0)
        .position(0.0, 0.0)
        .visible(false)
        .on_navigation(move |u| u.host_str() == Some(host.as_str())) // a redirect elsewhere is not followed
        .on_page_load(move |_, p| {
            if matches!(p.event(), PageLoadEvent::Finished) {
                let _ = loaded_tx.send(());
            }
        })
        .build()
        .map_err(|e| format!("could not open the capture view: {e}"))?;
    // A window that is off-screen or ordered out counts as occluded: WebKit then pauses animation frames and timers, and
    // a page that reveals its content from script paints blank. So the window sits on screen but fully transparent,
    // behind everything, and ignores the mouse; the snapshot renders from the view regardless of its alpha.
    make_invisible_but_live(&win);
    let result = (|| {
        wait_until_renderable(&win, &loaded_rx)?;
        // Pages that reveal themselves from script (an intro animation, a fade-in on hydration) are blank at first.
        // Snapshot until two successive frames match (the page has settled) or a few drawn frames have been seen;
        // a page that stays one flat colour is reported rather than shown as if it were a preview.
        let mut best: Option<Vec<u8>> = None;
        let mut drawn = 0;
        let started = std::time::Instant::now();
        let png = loop {
            let (png, blank) = snapshot(&win)?;
            if !blank {
                drawn += 1;
                if best.as_ref() == Some(&png) || drawn >= 4 {
                    break png;
                }
                best = Some(png);
            }
            if started.elapsed() > SETTLE_LIMIT {
                break best.ok_or_else(|| "The page stayed blank for 12 seconds after loading; it may be waiting on a script or an animation. Open the app to inspect it.".to_string())?;
            }
            std::thread::sleep(Duration::from_millis(1000));
        };
        use base64_lite::encode;
        Ok(format!("data:image/png;base64,{}", encode(&png)))
    })();
    let _ = win.destroy();
    result
}

#[cfg(target_os = "macos")]
fn make_invisible_but_live(win: &tauri::WebviewWindow) {
    use objc2_app_kit::NSWindow;
    if let Ok(ptr) = win.ns_window() {
        // SAFETY: `ns_window` is the live NSWindow of this window; Tauri runs window calls that need it on the main
        // thread, and these setters are only invoked from it through `run_on_main_thread`.
        let addr = ptr as usize;
        let _ = win.run_on_main_thread(move || unsafe {
            let w: &NSWindow = &*(addr as *const NSWindow);
            w.setAlphaValue(0.0);
            w.setIgnoresMouseEvents(true);
            w.orderBack(None);
        });
    }
    let _ = win.show();
}

/// Whether `document.readyState:<characters of visible text>` says there is something worth capturing: a finished
/// document, or one whose text has already rendered even though the parser is still waiting on something (a stalled
/// third-party request can hold `readyState` at `loading` for ever on a page that is fully drawn). A page with next to
/// no text yet is not ready.
fn ready_enough(report: &str) -> bool {
    const RENDERED: u64 = 200;
    let (state, text) = report.split_once(':').unwrap_or((report, "0"));
    let text = text.parse::<u64>().unwrap_or(0);
    match state {
        "complete" => true,
        "interactive" => text > 0,
        "loading" => text >= RENDERED,
        _ => false,
    }
}

/// The page's own load event waits for every subresource, so one slow third-party request (a font, an embedded frame,
/// an API call) can hold it back for ever even though the page is fully drawn. Wait for the load event, but once the
/// document itself is ready enough, stop waiting for the rest.
#[cfg(target_os = "macos")]
fn wait_until_renderable(win: &tauri::WebviewWindow, loaded: &mpsc::Receiver<()>) -> Result<(), String> {
    const DEADLINE: Duration = Duration::from_secs(25);
    let start = std::time::Instant::now();
    let mut last = String::from("no response");
    while start.elapsed() < DEADLINE {
        if loaded.recv_timeout(Duration::from_millis(500)).is_ok() {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(3) {
            if let Some(report) = document_report(win) {
                if ready_enough(&report) {
                    return Ok(());
                }
                last = report;
            }
        }
    }
    Err(format!("The page's document was still loading after {} seconds (last state: {last}).", DEADLINE.as_secs()))
}

/// `readyState:text-length` of the page, or `None` when it could not be read.
#[cfg(target_os = "macos")]
fn document_report(win: &tauri::WebviewWindow) -> Option<String> {
    use block2::RcBlock;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSError, NSString};
    use objc2_web_kit::WKWebView;
    let (tx, rx) = mpsc::channel::<Option<String>>();
    win.with_webview(move |wv| {
        // SAFETY: as in `capture`: `inner()` is the live WKWebView and this runs on the main thread.
        unsafe {
            let wk: &WKWebView = &*(wv.inner() as *const WKWebView);
            let js = NSString::from_str("document.readyState + ':' + (document.body ? document.body.innerText.trim().length : 0)");
            let block = RcBlock::new(move |res: *mut AnyObject, _err: *mut NSError| {
                let text = res.as_ref().and_then(|o| o.downcast_ref::<NSString>()).map(|s| s.to_string());
                let _ = tx.send(text);
            });
            wk.evaluateJavaScript_completionHandler(&js, Some(&block));
        }
    })
    .ok()?;
    rx.recv_timeout(Duration::from_secs(3)).ok().flatten()
}

/// Standard base64; the app has no encoder dependency and this is the only use.
mod base64_lite {
    pub fn encode(b: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() { s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char) } else { s.push('=') }
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_local_gateway_and_loopback_are_captured() {
        for ok in ["http://z32rp69g8b.harpoon.localhost:7480", "http://127.0.0.1:39000/x", "http://localhost:3000"] {
            assert!(loopback_target(ok).is_ok(), "{ok}");
        }
        for bad in ["https://z.harpoon.localhost", "http://example.com", "http://evilharpoon.localhost", "http://u:p@127.0.0.1", "file:///etc/passwd", "http://127.0.0.1.evil.com", "nonsense"] {
            assert!(loopback_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn upgrade_insecure_requests_is_found_only_in_an_enforced_policy() {
        assert!(csp_upgrades("HTTP/1.1 200 OK\r\nContent-Security-Policy: default-src 'self'; upgrade-insecure-requests\r\nX: y"));
        assert!(!csp_upgrades("HTTP/1.1 200 OK\r\nContent-Security-Policy: default-src 'self'; frame-ancestors 'none'"));
        assert!(!csp_upgrades("HTTP/1.1 200 OK\r\nContent-Security-Policy-Report-Only: upgrade-insecure-requests"));
        assert!(!csp_upgrades("HTTP/1.1 200 OK\r\nX-Note: upgrade-insecure-requests"));
    }

    #[test]
    fn a_page_is_ready_when_complete_or_parsed_with_visible_text() {
        assert!(ready_enough("complete:0"));
        assert!(ready_enough("interactive:420"));
        assert!(!ready_enough("interactive:0"), "parsed but nothing drawn yet");
        assert!(ready_enough("loading:5341"), "text is on screen though the parser is still waiting");
        assert!(!ready_enough("loading:12"), "next to nothing drawn yet");
        assert!(!ready_enough("garbage"));
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64_lite::encode(b""), "");
        assert_eq!(base64_lite::encode(b"f"), "Zg==");
        assert_eq!(base64_lite::encode(b"fo"), "Zm8=");
        assert_eq!(base64_lite::encode(b"foo"), "Zm9v");
        assert_eq!(base64_lite::encode(b"foobar"), "Zm9vYmFy");
    }
}
