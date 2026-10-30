//! The window's title bar in the app's own colours (owner, 2026-10-30: "I want this window bar to
//! be the same color as the application").
//!
//! The bar stays the native Windows one (snap layouts, the system menu and the min/max/close
//! buttons keep working); Windows 11's DWM is only told what to paint it with. Colours are the web
//! app's `--bg` and `--text` (apps/web/src/index.css), which is dark only. The border is set to
//! `--bg` too, so the system accent colour no longer draws a blue line along the window's edge.
//!
//! Windows 10 has no caption/border colour attributes: those calls fail and the bar stays as it
//! was. Nothing here can stop the app from starting.

/// `0x00BBGGRR`, the DWM `COLORREF` layout.
const fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}

/// `--bg`.
const BACKGROUND: u32 = colorref(0x0f1216);
/// `--text`.
const TEXT: u32 = colorref(0xe4e8ed);

#[cfg(windows)]
pub fn apply(window: &tauri::WebviewWindow) {
    use std::ffi::c_void;

    // dwmapi.h
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    const DWMWA_TEXT_COLOR: u32 = 36;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attr: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    let hwnd = match window.hwnd() {
        Ok(h) => h.0,
        Err(e) => {
            log::warn!("title bar colours: no window handle: {e}");
            return;
        }
    };
    let set = |attr: u32, value: u32| {
        // SAFETY: `hwnd` is this process's live top-level window; `value` outlives the call and
        // is the 4-byte BOOL/COLORREF every one of these attributes takes.
        let hr = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attr,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        };
        if hr < 0 {
            log::info!("title bar colours: attribute {attr} not supported here (HRESULT {hr:#x})");
        }
    };
    // Dark caption buttons first (also the whole fallback where the colours below are unsupported).
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    set(DWMWA_CAPTION_COLOR, BACKGROUND);
    set(DWMWA_TEXT_COLOR, TEXT);
    set(DWMWA_BORDER_COLOR, BACKGROUND);
}

#[cfg(not(windows))]
pub fn apply(_window: &tauri::WebviewWindow) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_swaps_red_and_blue() {
        assert_eq!(colorref(0x0f1216), 0x16120f);
        assert_eq!(colorref(0xe4e8ed), 0xede8e4);
    }
}
