//! The primary selection and the clipboard, for middle-click and the
//! search box (see `plot/paste.rs`). Only the app reads them: the
//! embeddable `Plot` has no clipboard dependency.
//!
//! On X11 (also over `ssh -X`) both come through arboard. On Wayland
//! they come through smithay-clipboard on the app's own Wayland
//! connection, as egui-winit reads its clipboard; arboard alone would
//! need the data-control protocol, which GNOME lacks. Elsewhere (macOS,
//! Windows), and on a Wayland compositor without the primary-selection
//! protocol, there is no primary selection, and middle-click reads the
//! clipboard.
//!
//! A read can be slow (X11 asks the owner, which may hang, for up to 4 s
//! per format) so the app never reads on its own thread: a [`Worker`]
//! owns the selection on a thread of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Where the app reads and sets the selection. Tests give their own.
pub trait Selection {
    /// There is a primary selection here (X11, or a Wayland compositor
    /// with the protocol). Without one, middle-click reads the clipboard.
    fn has_primary(&mut self) -> bool;
    /// The primary selection's text, `None` when nothing is selected.
    fn primary(&mut self) -> Option<String>;
    /// The clipboard's text.
    fn clipboard(&mut self) -> Option<String>;
    /// Make `text` the primary selection, where there is one.
    fn set_primary(&mut self, text: &str);

    /// What a middle-click pastes: the primary selection, or the clipboard
    /// where there is no primary selection (not merely an empty one, so
    /// a middle-click with nothing selected never pastes something copied
    /// long ago).
    fn middle_click_text(&mut self) -> Option<String> {
        if self.has_primary() {
            self.primary()
        } else {
            self.clipboard()
        }
    }
}

/// Makes the selection on the worker's thread.
pub type MakeSelection = Box<dyn FnOnce() -> Box<dyn Selection> + Send>;

enum Job<T> {
    Read(T),
    Set(String),
}

/// A [`Selection`] on a thread of its own: reads answer later, tagged
/// with what asked, and nothing ever blocks the caller.
///
/// Jobs run one at a time, so a read stuck on a hung selection owner
/// holds up those after it: each of those clicks times out in the app
/// with its notice, until the owner answers or goes.
pub struct Worker<T> {
    jobs: Option<mpsc::Sender<Job<T>>>,
    answers: mpsc::Receiver<(T, Option<String>)>,
    done: mpsc::Receiver<()>,
    /// The worker was given up on (stuck past [`Worker`]'s drop): it must
    /// not touch the selection again, as the display may be gone.
    abandoned: Arc<AtomicBool>,
}

impl<T: Send + 'static> Worker<T> {
    /// Start the thread, making the selection there with `make`; `wake`
    /// is called after each answer (to repaint).
    pub fn spawn(make: MakeSelection, wake: impl Fn() + Send + 'static) -> Self {
        let (jobs, rx) = mpsc::channel::<Job<T>>();
        let (tx, answers) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let given_up = abandoned.clone();
        let spawned = std::thread::Builder::new()
            .name("archiveview-selection".into())
            .spawn(move || {
                let mut sel = make();
                for job in rx {
                    if given_up.load(Ordering::Acquire) {
                        break;
                    }
                    match job {
                        Job::Read(tag) => {
                            let text = sel.middle_click_text();
                            if tx.send((tag, text)).is_err() {
                                break;
                            }
                            wake();
                        }
                        Job::Set(text) => sel.set_primary(&text),
                    }
                }
                if given_up.load(Ordering::Acquire) {
                    // Dropping it now would close its Wayland queue on a
                    // display that may be gone: leak it, the process is
                    // ending.
                    log::warn!("the selection was stuck at exit; leaving it");
                    std::mem::forget(sel);
                } else {
                    drop(sel);
                    let _ = done_tx.send(());
                }
            });
        if let Err(e) = spawned {
            log::warn!("no selection thread: {e}");
        }
        Self {
            jobs: Some(jobs),
            answers,
            done,
            abandoned,
        }
    }

    /// Read what a middle-click pastes; the answer comes with `tag`.
    pub fn read(&self, tag: T) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Read(tag));
        }
    }

    /// Make `text` the primary selection.
    pub fn set_primary(&self, text: String) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Set(text));
        }
    }

    /// An answer, if one has come.
    pub fn answer(&self) -> Option<(T, Option<String>)> {
        self.answers.try_recv().ok()
    }
}

impl<T> Drop for Worker<T> {
    /// Stop the thread and wait (a little) for it to drop the selection,
    /// which on Wayland must go before the display does. A thread stuck
    /// in a read (smithay's has no timeout) is given up on: it leaks the
    /// selection when it wakes, rather than racing the display's teardown.
    fn drop(&mut self) {
        self.jobs = None;
        if self.done.recv_timeout(Duration::from_secs(1)).is_err() {
            log::warn!("the selection thread is stuck; abandoning it");
            self.abandoned.store(true, Ordering::Release);
        }
    }
}

/// The system's selection, for [`Worker::spawn`]: on Wayland through
/// `display`, else (X11, other platforms) through arboard.
///
/// # Safety
///
/// A Wayland `display` must stay valid while the selection lives: drop
/// its worker before the window goes (`eframe::App::on_exit`).
#[cfg(not(target_arch = "wasm32"))]
pub unsafe fn system(display: Option<raw_window_handle::RawDisplayHandle>) -> MakeSelection {
    /// The display pointer, sent to the worker's thread.
    struct Display(Option<raw_window_handle::RawDisplayHandle>);
    // SAFETY: only used to open the clipboard's own Wayland queue, which
    // libwayland allows from any thread.
    unsafe impl Send for Display {}
    let display = Display(display);
    Box::new(move || {
        let display = display;
        // SAFETY: the caller keeps the display alive while we live.
        Box::new(unsafe { SystemSelection::new(display.0) })
    })
}

/// The system's primary selection and clipboard.
#[cfg(not(target_arch = "wasm32"))]
pub struct SystemSelection {
    wayland: Option<platform::Wayland>,
    arboard: Option<arboard::Clipboard>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SystemSelection {
    /// # Safety
    ///
    /// As [`system`].
    unsafe fn new(display: Option<raw_window_handle::RawDisplayHandle>) -> Self {
        // SAFETY: passed on from the caller.
        if let Some(wayland) = unsafe { platform::Wayland::new(display) } {
            return Self {
                wayland: Some(wayland),
                arboard: None,
            };
        }
        let arboard = arboard::Clipboard::new()
            .inspect_err(|e| log::warn!("no clipboard: {e}"))
            .ok();
        Self {
            wayland: None,
            arboard,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Selection for SystemSelection {
    fn has_primary(&mut self) -> bool {
        match &self.wayland {
            Some(w) => w.has_primary(),
            None => platform::HAS_PRIMARY && self.arboard.is_some(),
        }
    }

    fn primary(&mut self) -> Option<String> {
        let text = match &self.wayland {
            Some(w) => w.load_primary(),
            None => platform::arboard_primary(self.arboard.as_mut()?),
        };
        text.filter(|t| !t.is_empty())
    }

    fn clipboard(&mut self) -> Option<String> {
        match &self.wayland {
            Some(w) => w.load(),
            None => self.arboard.as_mut()?.get_text().ok(),
        }
    }

    fn set_primary(&mut self, text: &str) {
        match &self.wayland {
            Some(w) => w.store_primary(text),
            None => {
                if let Some(a) = self.arboard.as_mut() {
                    platform::arboard_set_primary(a, text);
                }
            }
        }
    }
}

/// The platforms with a primary selection: those egui-winit reads the
/// Wayland clipboard on.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod platform {
    use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};
    use raw_window_handle::RawDisplayHandle;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};

    pub const HAS_PRIMARY: bool = true;

    pub fn arboard_primary(a: &mut arboard::Clipboard) -> Option<String> {
        a.get().clipboard(LinuxClipboardKind::Primary).text().ok()
    }

    pub fn arboard_set_primary(a: &mut arboard::Clipboard, text: &str) {
        if let Err(e) = a.set().clipboard(LinuxClipboardKind::Primary).text(text) {
            log::warn!("could not set the primary selection: {e}");
        }
    }

    /// The selection on a Wayland connection.
    pub struct Wayland {
        clipboard: smithay_clipboard::Clipboard,
        primary: bool,
    }

    impl Wayland {
        /// # Safety
        ///
        /// A Wayland `display` must stay valid while this lives.
        pub unsafe fn new(display: Option<RawDisplayHandle>) -> Option<Self> {
            let Some(RawDisplayHandle::Wayland(d)) = display else {
                return None;
            };
            let ptr = d.display.as_ptr();
            // SAFETY: the caller keeps the display alive while we live.
            let primary = unsafe { has_primary_protocol(ptr) };
            // SAFETY: as above.
            let clipboard = unsafe { smithay_clipboard::Clipboard::new(ptr) };
            Some(Self { clipboard, primary })
        }

        pub fn has_primary(&self) -> bool {
            self.primary
        }

        pub fn load_primary(&self) -> Option<String> {
            self.clipboard.load_primary().ok()
        }

        pub fn load(&self) -> Option<String> {
            self.clipboard.load().ok()
        }

        pub fn store_primary(&self, text: &str) {
            self.clipboard.store_primary(text);
        }
    }

    struct Probe;

    impl Dispatch<WlRegistry, GlobalListContents> for Probe {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    /// Whether the compositor offers the primary-selection protocol
    /// (smithay-clipboard's errors can't tell "unsupported" from "empty").
    ///
    /// # Safety
    ///
    /// `display` must be a live `wl_display`.
    unsafe fn has_primary_protocol(display: *mut std::ffi::c_void) -> bool {
        // SAFETY: the caller's.
        let backend =
            unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        match registry_queue_init::<Probe>(&conn) {
            Ok((globals, _queue)) => globals.contents().with_list(|list| {
                list.iter()
                    .any(|g| g.interface == "zwp_primary_selection_device_manager_v1")
            }),
            Err(e) => {
                log::warn!("could not list Wayland globals: {e}");
                true
            }
        }
    }
}

/// The platforms without a primary selection.
#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
mod platform {
    pub const HAS_PRIMARY: bool = false;

    pub fn arboard_primary(_: &mut arboard::Clipboard) -> Option<String> {
        None
    }

    pub fn arboard_set_primary(_: &mut arboard::Clipboard, _: &str) {}

    /// No Wayland here.
    pub enum Wayland {}

    impl Wayland {
        /// # Safety
        ///
        /// None needed: there is no Wayland here.
        pub unsafe fn new(_: Option<raw_window_handle::RawDisplayHandle>) -> Option<Self> {
            None
        }

        pub fn has_primary(&self) -> bool {
            match *self {}
        }

        pub fn load_primary(&self) -> Option<String> {
            match *self {}
        }

        pub fn load(&self) -> Option<String> {
            match *self {}
        }

        pub fn store_primary(&self, _: &str) {
            match *self {}
        }
    }
}
