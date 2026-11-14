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
fn loopback_target(url: &str) -> Result<tauri::Url, String> {
    let u: tauri::Url = url.parse().map_err(|_| "The application address is not a valid URL.".to_string())?;
    let host = u.host_str().unwrap_or("");
    let local = host == "localhost" || host == "127.0.0.1" || host == "harpoon.localhost" || host.ends_with(".harpoon.localhost");
    if u.scheme() != "http" || !local || !u.username().is_empty() || u.password().is_some() {
        return Err("A preview is only taken of an application on this Mac (http, no credentials in the address).".into());
    }
    Ok(u)
}

#[cfg(target_os = "macos")]
fn png_of(img: &objc2_app_kit::NSImage) -> Result<Vec<u8>, String> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::NSDictionary;
    let tiff = img.TIFFRepresentation().ok_or("the snapshot has no image data")?;
    let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff).ok_or("the snapshot could not be read")?;
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
        .ok_or("the snapshot could not be encoded")?;
    Ok(png.to_vec())
}

/// `data:image/png;base64,…` of the page at `url`, or the reason none could be made.
#[tauri::command]
pub async fn deploy_preview(app: tauri::AppHandle, url: String) -> Result<String, String> {
    let target = loopback_target(&url)?;
    tauri::async_runtime::spawn_blocking(move || capture(&app, target)).await.map_err(|e| format!("background task failed: {e}"))?
}

#[cfg(not(target_os = "macos"))]
fn capture(_app: &tauri::AppHandle, _target: tauri::Url) -> Result<String, String> {
    Err("Page previews are available on macOS only.".into())
}

#[cfg(target_os = "macos")]
fn capture(app: &tauri::AppHandle, target: tauri::Url) -> Result<String, String> {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSImage;
    use objc2_foundation::{NSError, NSNumber};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
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
        .position(-20000.0, -20000.0)
        .on_navigation(move |u| u.host_str() == Some(host.as_str())) // a redirect elsewhere is not followed
        .on_page_load(move |_, p| {
            if matches!(p.event(), PageLoadEvent::Finished) {
                let _ = loaded_tx.send(());
            }
        })
        .build()
        .map_err(|e| format!("could not open the capture view: {e}"))?;
    let result = (|| {
        loaded_rx.recv_timeout(Duration::from_secs(20)).map_err(|_| "The application did not finish loading within 20 seconds.".to_string())?;
        std::thread::sleep(Duration::from_millis(1500)); // let client-side rendering settle
        let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
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
        let png = rx.recv_timeout(Duration::from_secs(15)).map_err(|_| "The capture did not complete.".to_string())??;
        use base64_lite::encode;
        Ok(format!("data:image/png;base64,{}", encode(&png)))
    })();
    let _ = win.destroy();
    result
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
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64_lite::encode(b""), "");
        assert_eq!(base64_lite::encode(b"f"), "Zg==");
        assert_eq!(base64_lite::encode(b"fo"), "Zm8=");
        assert_eq!(base64_lite::encode(b"foo"), "Zm9v");
        assert_eq!(base64_lite::encode(b"foobar"), "Zm9vYmFy");
    }
}
