//! An [`Editor`] implementation for egui.

use crate::egui::Vec2;
use crate::egui::ViewportCommand;
use crate::EguiState;
use baseview::gl::GlConfig;
use baseview::PhySize;
use baseview::{Size, WindowHandle, WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use egui_baseview::egui::Context;
use egui_baseview::EguiWindow;
use nih_plug::prelude::{Editor, GuiContext, ParamSetter, ParentWindowHandle};
use parking_lot::RwLock;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::any::Any;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[cfg(target_os = "linux")]
#[repr(C)]
struct X11Display {
    _private: [u8; 0],
}

#[cfg(target_os = "linux")]
#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(display_name: *const std::ffi::c_char) -> *mut X11Display;
    fn XMapWindow(display: *mut X11Display, window: std::ffi::c_ulong) -> std::ffi::c_int;
    fn XUnmapWindow(display: *mut X11Display, window: std::ffi::c_ulong) -> std::ffi::c_int;
    fn XSync(display: *mut X11Display, discard: std::ffi::c_int) -> std::ffi::c_int;
    fn XCloseDisplay(display: *mut X11Display) -> std::ffi::c_int;
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn ShowWindow(window: *mut std::ffi::c_void, command: std::ffi::c_int) -> std::ffi::c_int;
}

#[cfg(target_os = "macos")]
#[cfg(target_arch = "aarch64")]
type ObjcBool = bool;

#[cfg(target_os = "macos")]
#[cfg(target_arch = "x86_64")]
type ObjcBool = i8;

#[cfg(target_os = "macos")]
#[link(name = "objc")]
unsafe extern "C" {
    fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    #[link_name = "objc_msgSend"]
    fn objc_msg_send_set_hidden(
        receiver: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
        hidden: ObjcBool,
    );
}

fn set_native_window_visible(window: &mut WindowHandle, visible: bool) -> bool {
    match window.raw_window_handle() {
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(handle) if handle.window != 0 => {
            // CLAP invokes Editor::set_visible() on the host's main thread. Open a short-lived
            // Xlib connection to map only the already-created child window; keep its baseview
            // handle and egui state alive across hide/show.
            // SAFETY: a null display name asks Xlib to open the display named by DISPLAY.
            let display = unsafe { XOpenDisplay(std::ptr::null()) };
            if display.is_null() {
                return false;
            }
            // SAFETY: the native X11 child belongs to this live baseview handle. This function is
            // called by the host on its GUI thread, and the display remains open through XSync.
            let changed = unsafe {
                let changed = if visible {
                    XMapWindow(display, handle.window)
                } else {
                    XUnmapWindow(display, handle.window)
                };
                XSync(display, 0);
                XCloseDisplay(display);
                changed != 0
            };
            changed
        }
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(handle) if !handle.hwnd.is_null() => {
            const SW_HIDE: i32 = 0;
            const SW_SHOW: i32 = 5;
            // ShowWindow returns the prior visibility state, so a zero result is valid when an
            // already-hidden/shown child remains in the requested state.
            // SAFETY: baseview owns this live HWND and CLAP calls visibility changes on the GUI
            // thread. The return value is the old visibility state, not an error code.
            unsafe { ShowWindow(handle.hwnd, if visible { SW_SHOW } else { SW_HIDE }) };
            true
        }
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(handle) if !handle.ns_view.is_null() => {
            // The embedded child is an NSView. Toggle its hidden flag without releasing the view
            // or rebuilding the egui editor.
            // SAFETY: the handle is baseview's live child NSView and CLAP calls this on the main
            // thread, as required for AppKit view updates.
            unsafe {
                let selector = sel_registerName(c"setHidden:".as_ptr());
                #[cfg(target_arch = "aarch64")]
                let hidden = !visible;
                #[cfg(target_arch = "x86_64")]
                let hidden = i8::from(!visible);
                objc_msg_send_set_hidden(handle.ns_view, selector, hidden);
            }
            true
        }
        _ => false,
    }
}

/// An [`Editor`] implementation that calls an egui draw loop.
pub(crate) struct EguiEditor<T> {
    pub(crate) egui_state: Arc<EguiState>,
    /// The plugin's state. This is kept in between editor openenings.
    pub(crate) user_state: Arc<RwLock<T>>,

    /// The user's build function. Applied once at the start of the application.
    pub(crate) build: Arc<dyn Fn(&Context, &mut T) + 'static + Send + Sync>,
    /// The user's update function.
    pub(crate) update: Arc<dyn Fn(&Context, &ParamSetter, &mut T) + 'static + Send + Sync>,

    /// The scaling factor reported by the host, if any. On macOS this will never be set and we
    /// should use the system scaling factor instead.
    pub(crate) scaling_factor: AtomicCell<Option<f32>>,
}

/// This version of `baseview` uses a different version of `raw_window_handle than NIH-plug, so we
/// need to adapt it ourselves.
struct ParentWindowHandleAdapter(nih_plug::editor::ParentWindowHandle);

unsafe impl HasRawWindowHandle for ParentWindowHandleAdapter {
    fn raw_window_handle(&self) -> RawWindowHandle {
        match self.0 {
            ParentWindowHandle::X11Window(window) => {
                let mut handle = raw_window_handle::XcbWindowHandle::empty();
                handle.window = window;
                RawWindowHandle::Xcb(handle)
            }
            ParentWindowHandle::AppKitNsView(ns_view) => {
                let mut handle = raw_window_handle::AppKitWindowHandle::empty();
                handle.ns_view = ns_view;
                RawWindowHandle::AppKit(handle)
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                let mut handle = raw_window_handle::Win32WindowHandle::empty();
                handle.hwnd = hwnd;
                RawWindowHandle::Win32(handle)
            }
        }
    }
}

impl<T> Editor for EguiEditor<T>
where
    T: 'static + Send + Sync,
{
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let build = self.build.clone();
        let update = self.update.clone();
        let state = self.user_state.clone();
        let egui_state = self.egui_state.clone();

        let (unscaled_width, unscaled_height) = self.egui_state.size();
        let scaling_factor = self.scaling_factor.load();
        let window = EguiWindow::open_parented(
            &ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("egui window"),
                // Baseview should be doing the DPI scaling for us
                size: Size::new(unscaled_width as f64, unscaled_height as f64),
                // NOTE: For some reason passing 1.0 here causes the UI to be scaled on macOS but
                //       not the mouse events.
                scale: scaling_factor
                    .map(|factor| WindowScalePolicy::ScaleFactor(factor as f64))
                    .unwrap_or(WindowScalePolicy::SystemScaleFactor),

                #[cfg(feature = "opengl")]
                gl_config: Some(GlConfig {
                    version: (3, 2),
                    red_bits: 8,
                    blue_bits: 8,
                    green_bits: 8,
                    alpha_bits: 8,
                    depth_bits: 24,
                    stencil_bits: 8,
                    samples: None,
                    srgb: true,
                    double_buffer: true,
                    vsync: true,
                    ..Default::default()
                }),
            },
            Default::default(),
            state,
            move |egui_ctx, _queue, state| build(egui_ctx, &mut state.write()),
            move |egui_ctx, queue, state| {
                let setter = ParamSetter::new(context.as_ref());

                // If the window was requested to resize
                if let Some(new_size) = egui_state.requested_size.swap(None) {
                    // Ask the plugin host to resize to self.size()
                    if context.request_resize() {
                        // Resize the content of egui window
                        queue.resize(PhySize::new(new_size.0, new_size.1));
                        egui_ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                            new_size.0 as f32,
                            new_size.1 as f32,
                        )));

                        // Update the state
                        egui_state.size.store(new_size);
                    }
                }

                // For now, just always redraw. Most plugin GUIs have meters, and those almost always
                // need a redraw. Later we can try to be a bit more sophisticated about this. Without
                // this we would also have a blank GUI when it gets first opened because most DAWs open
                // their GUI while the window is still unmapped.
                egui_ctx.request_repaint();
                (update)(egui_ctx, &setter, &mut state.write());
            },
        );

        self.egui_state.open.store(true, Ordering::Release);
        Box::new(EguiEditorHandle {
            egui_state: self.egui_state.clone(),
            window,
        })
    }

    /// Size of the editor window
    fn size(&self) -> (u32, u32) {
        let new_size = self.egui_state.requested_size.load();
        // This method will be used to ask the host for new size.
        // If the editor is currently being resized and new size hasn't been consumed and set yet, return new requested size.
        if let Some(new_size) = new_size {
            new_size
        } else {
            self.egui_state.size()
        }
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        // If the editor is currently open then the host must not change the current HiDPI scale as
        // we don't have a way to handle that. Ableton Live does this.
        if self.egui_state.is_open() {
            return false;
        }

        self.scaling_factor.store(Some(factor));
        true
    }

    fn set_visible(&self, editor_handle: &mut dyn Any, visible: bool) -> bool {
        let Some(editor_handle) = editor_handle.downcast_mut::<EguiEditorHandle>() else {
            return false;
        };
        set_native_window_visible(&mut editor_handle.window, visible)
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        // As mentioned above, for now we'll always force a redraw to allow meter widgets to work
        // correctly. In the future we can use an `Arc<AtomicBool>` and only force a redraw when
        // that boolean is set.
    }

    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}

    fn param_values_changed(&self) {
        // Same
    }
}

/// The window handle used for [`EguiEditor`].
struct EguiEditorHandle {
    egui_state: Arc<EguiState>,
    window: WindowHandle,
}

/// The window handle enum stored within 'WindowHandle' contains raw pointers. Is there a way around
/// having this requirement?
unsafe impl Send for EguiEditorHandle {}

impl Drop for EguiEditorHandle {
    fn drop(&mut self) {
        self.egui_state.open.store(false, Ordering::Release);
        // XXX: This should automatically happen when the handle gets dropped, but apparently not
        self.window.close();
    }
}
