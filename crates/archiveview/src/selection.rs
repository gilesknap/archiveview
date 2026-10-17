//! The primary selection and the clipboard, for middle-click and the
//! search box (see `plot/paste.rs`). Only the app reads them: the
//! embeddable `Plot` has no clipboard dependency.
//!
//! On X11 (also over `ssh -X`) both come through arboard. On Wayland
//! they come through smithay-clipboard on the app's own Wayland
//! connection, as egui-winit reads its clipboard; arboard alone would
//! need the data-control protocol, which GNOME lacks. Elsewhere (macOS,
//! Windows) there is no primary selection, and middle-click reads the
//! clipboard.

/// Where the app reads and sets the selection. Tests give their own.
pub trait Selection {
    /// The primary selection's text, `None` where there is none (no
    /// primary selection on this platform or compositor, or nothing
    /// selected): then middle-click reads the clipboard.
    fn primary(&mut self) -> Option<String>;
    /// The clipboard's text.
    fn clipboard(&mut self) -> Option<String>;
    /// Make `text` the primary selection, where there is one.
    fn set_primary(&mut self, text: &str);
}

/// The system's primary selection and clipboard.
#[cfg(not(target_arch = "wasm32"))]
pub struct SystemSelection {
    #[cfg(all(
        unix,
        not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "emscripten"
        ))
    ))]
    wayland: Option<smithay_clipboard::Clipboard>,
    arboard: Option<arboard::Clipboard>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SystemSelection {
    /// Read the selection through `display` when it is a Wayland
    /// connection, else (X11, other platforms) through arboard.
    ///
    /// # Safety
    ///
    /// A Wayland `display` must stay valid while this lives: drop it
    /// before the window goes (`eframe::App::on_exit`).
    pub unsafe fn new(display: Option<raw_window_handle::RawDisplayHandle>) -> Self {
        #[cfg(all(
            unix,
            not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "emscripten"
            ))
        ))]
        if let Some(raw_window_handle::RawDisplayHandle::Wayland(d)) = display {
            // SAFETY: the caller keeps the display alive while we live.
            let clipboard = unsafe { smithay_clipboard::Clipboard::new(d.display.as_ptr()) };
            return Self {
                wayland: Some(clipboard),
                arboard: None,
            };
        }
        let _ = display;
        let arboard = arboard::Clipboard::new()
            .inspect_err(|e| log::warn!("no clipboard: {e}"))
            .ok();
        Self {
            #[cfg(all(
                unix,
                not(any(
                    target_os = "macos",
                    target_os = "ios",
                    target_os = "android",
                    target_os = "emscripten"
                ))
            ))]
            wayland: None,
            arboard,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Selection for SystemSelection {
    fn primary(&mut self) -> Option<String> {
        #[cfg(all(
            unix,
            not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "emscripten"
            ))
        ))]
        {
            if let Some(w) = &self.wayland {
                return w.load_primary().ok().filter(|t| !t.is_empty());
            }
            use arboard::{GetExtLinux, LinuxClipboardKind};
            let a = self.arboard.as_mut()?;
            a.get()
                .clipboard(LinuxClipboardKind::Primary)
                .text()
                .ok()
                .filter(|t| !t.is_empty())
        }
        #[cfg(not(all(
            unix,
            not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "emscripten"
            ))
        )))]
        None
    }

    fn clipboard(&mut self) -> Option<String> {
        #[cfg(all(
            unix,
            not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "emscripten"
            ))
        ))]
        if let Some(w) = &self.wayland {
            return w.load().ok();
        }
        self.arboard.as_mut()?.get_text().ok()
    }

    fn set_primary(&mut self, text: &str) {
        #[cfg(all(
            unix,
            not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "emscripten"
            ))
        ))]
        {
            if let Some(w) = &self.wayland {
                w.store_primary(text);
                return;
            }
            use arboard::{LinuxClipboardKind, SetExtLinux};
            if let Some(a) = self.arboard.as_mut()
                && let Err(e) = a.set().clipboard(LinuxClipboardKind::Primary).text(text)
            {
                log::warn!("could not set the primary selection: {e}");
            }
        }
        #[cfg(not(all(
            unix,
            not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "emscripten"
            ))
        )))]
        let _ = text;
    }
}
