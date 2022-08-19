// Copyright © SixtyFPS GmbH <info@slint-ui.com>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-commercial

#![warn(missing_docs)]
/*!
    This module contains the event loop implementation using winit, as well as the
    [PlatformWindow] trait used by the generated code and the run-time to change
    aspects of windows on the screen.
*/
use copypasta::ClipboardProvider;
use corelib::items::PointerEventButton;
use corelib::lengths::LogicalSize;
use i_slint_core as corelib;

use corelib::api::euclid;
use corelib::graphics::Point;
use corelib::input::{KeyEvent, KeyEventType, KeyboardModifiers, MouseEvent};
use corelib::window::*;
use corelib::{Coord, SharedString};
use std::cell::{Cell, RefCell, RefMut};
use std::rc::{Rc, Weak};
use winit::event::WindowEvent;

#[cfg(not(target_arch = "wasm32"))]
use winit::platform::run_return::EventLoopExtRunReturn;

pub trait WinitWindow: PlatformWindow {
    fn runtime_window(&self) -> Rc<corelib::window::WindowInner>;
    fn currently_pressed_key_code(&self) -> &Cell<Option<winit::event::VirtualKeyCode>>;
    fn current_keyboard_modifiers(&self) -> &Cell<KeyboardModifiers>;
    fn draw(self: Rc<Self>);
    fn with_window_handle(&self, callback: &mut dyn FnMut(&winit::window::Window));
    fn constraints(&self) -> (corelib::layout::LayoutInfo, corelib::layout::LayoutInfo);
    fn set_constraints(
        &self,
        constraints: (corelib::layout::LayoutInfo, corelib::layout::LayoutInfo),
    );
    fn set_icon(&self, icon: corelib::graphics::Image);
    /// Called by the event loop when a WindowEvent::Resized is received.
    fn resize_event(&self, _size: winit::dpi::PhysicalSize<u32>) {}

    fn apply_constraints(
        &self,
        constraints_horizontal: corelib::layout::LayoutInfo,
        constraints_vertical: corelib::layout::LayoutInfo,
    ) {
        self.with_window_handle(&mut |winit_window| {
            // If we're in fullscreen state, don't try to resize the window but maintain the surface
            // size we've been assigned to from the windowing system. Weston/Wayland don't like it
            // when we create a surface that's bigger than the screen due to constraints (#532).
            if winit_window.fullscreen().is_some() {
                return;
            }

            if (constraints_horizontal, constraints_vertical) != self.constraints() {
                let min_width = constraints_horizontal.min.min(constraints_horizontal.max) as f32;
                let min_height = constraints_vertical.min.min(constraints_vertical.max) as f32;
                let max_width = constraints_horizontal.max.max(constraints_horizontal.min) as f32;
                let max_height = constraints_vertical.max.max(constraints_vertical.min) as f32;

                let sf = self.runtime_window().scale_factor();

                winit_window.set_resizable(true);
                winit_window.set_min_inner_size(if min_width > 0. || min_height > 0. {
                    Some(winit::dpi::PhysicalSize::new(min_width * sf, min_height * sf))
                } else {
                    None
                });
                winit_window.set_max_inner_size(
                    if max_width < i32::MAX as f32 || max_height < i32::MAX as f32 {
                        Some(winit::dpi::PhysicalSize::new(
                            (max_width * sf).min(65535.),
                            (max_height * sf).min(65535.),
                        ))
                    } else {
                        None
                    },
                );
                self.set_constraints((constraints_horizontal, constraints_vertical));
                winit_window.set_resizable(min_width < max_width || min_height < max_height);

                #[cfg(target_arch = "wasm32")]
                {
                    // set_max_inner_size / set_min_inner_size don't work on wasm, so apply the size manually
                    let existing_size: winit::dpi::LogicalSize<f32> =
                        winit_window.inner_size().to_logical(sf as f64);
                    if !(min_width..=max_width).contains(&(existing_size.width))
                        || !(min_height..=max_height).contains(&(existing_size.height))
                    {
                        let new_size = winit::dpi::LogicalSize::new(
                            existing_size.width.min(max_width).max(min_width),
                            existing_size.height.min(max_height).max(min_height),
                        );
                        winit_window.set_inner_size(new_size);
                    }
                }
            }
        });
    }

    fn apply_window_properties(
        &self,
        window_item: core::pin::Pin<&i_slint_core::items::WindowItem>,
    ) {
        let title = window_item.title();
        let no_frame = window_item.no_frame();
        let icon = window_item.icon();
        let mut width = window_item.width() as f32;
        let mut height = window_item.height() as f32;

        self.set_icon(icon);

        let mut must_resize = false;

        self.with_window_handle(&mut |winit_window| {
            winit_window.set_title(&title);
            if no_frame && winit_window.fullscreen().is_none() {
                winit_window.set_decorations(false);
            } else {
                winit_window.set_decorations(true);
            }

            if width <= 0. || height <= 0. {
                must_resize = true;

                let winit_size = winit_window
                    .inner_size()
                    .to_logical(self.runtime_window().scale_factor() as f64);

                if width <= 0. {
                    width = winit_size.width;
                }
                if height <= 0. {
                    height = winit_size.height;
                }
            }

            let existing_size: LogicalSize =
                self.inner_size().cast() / self.runtime_window().scale();

            if (existing_size.width as f32 - width).abs() > 1.
                || (existing_size.height as f32 - height).abs() > 1.
            {
                // If we're in fullscreen state, don't try to resize the window but maintain the surface
                // size we've been assigned to from the windowing system. Weston/Wayland don't like it
                // when we create a surface that's bigger than the screen due to constraints (#532).
                if winit_window.fullscreen().is_none() {
                    winit_window.set_inner_size(winit::dpi::LogicalSize::new(width, height));
                }
            }
        });

        if must_resize {
            let win = i_slint_core::api::Window::from(self.runtime_window());
            let f = win.scale_factor().0;
            win.set_size(euclid::size2((width as f32 * f) as _, (height as f32 * f) as _));
        }
    }

    /// Return true if the proxy element used for input method has the focus
    fn input_method_focused(&self) -> bool {
        false
    }

    fn inner_size(&self) -> euclid::Size2D<u32, corelib::lengths::PhysicalPx>;
}

struct NotRunningEventLoop {
    clipboard: RefCell<Box<dyn copypasta::ClipboardProvider>>,
    instance: winit::event_loop::EventLoop<CustomEvent>,
    event_loop_proxy: winit::event_loop::EventLoopProxy<CustomEvent>,
}

impl NotRunningEventLoop {
    fn new() -> Self {
        let instance = winit::event_loop::EventLoop::with_user_event();
        let event_loop_proxy = instance.create_proxy();
        let clipboard = RefCell::new(create_clipboard(&instance));
        Self { clipboard, instance, event_loop_proxy }
    }
}

struct RunningEventLoop<'a> {
    event_loop_target: &'a winit::event_loop::EventLoopWindowTarget<CustomEvent>,
    event_loop_proxy: &'a winit::event_loop::EventLoopProxy<CustomEvent>,
    clipboard: &'a RefCell<Box<dyn copypasta::ClipboardProvider>>,
}

pub(crate) trait EventLoopInterface {
    fn event_loop_target(&self) -> &winit::event_loop::EventLoopWindowTarget<CustomEvent>;
    fn event_loop_proxy(&self) -> &winit::event_loop::EventLoopProxy<CustomEvent>;
    fn clipboard(&self) -> RefMut<'_, dyn ClipboardProvider>;
}

impl EventLoopInterface for NotRunningEventLoop {
    fn event_loop_target(&self) -> &winit::event_loop::EventLoopWindowTarget<CustomEvent> {
        &*self.instance
    }

    fn event_loop_proxy(&self) -> &winit::event_loop::EventLoopProxy<CustomEvent> {
        &self.event_loop_proxy
    }

    fn clipboard(&self) -> RefMut<'_, dyn ClipboardProvider> {
        RefMut::map(self.clipboard.borrow_mut(), |clipboard_box| clipboard_box.as_mut())
    }
}

impl<'a> EventLoopInterface for RunningEventLoop<'a> {
    fn event_loop_target(&self) -> &winit::event_loop::EventLoopWindowTarget<CustomEvent> {
        self.event_loop_target
    }

    fn event_loop_proxy(&self) -> &winit::event_loop::EventLoopProxy<CustomEvent> {
        self.event_loop_proxy
    }

    fn clipboard(&self) -> RefMut<'_, dyn ClipboardProvider> {
        RefMut::map(self.clipboard.borrow_mut(), |clipboard_box| clipboard_box.as_mut())
    }
}

thread_local! {
    static ALL_WINDOWS: RefCell<std::collections::HashMap<winit::window::WindowId, Weak<dyn WinitWindow>>> = RefCell::new(std::collections::HashMap::new());
    static MAYBE_LOOP_INSTANCE: RefCell<Option<NotRunningEventLoop>> = RefCell::new(Some(NotRunningEventLoop::new()));
}

scoped_tls_hkt::scoped_thread_local!(static CURRENT_WINDOW_TARGET : for<'a> &'a RunningEventLoop<'a>);

pub(crate) enum GlobalEventLoopProxyOrEventQueue {
    Proxy(winit::event_loop::EventLoopProxy<CustomEvent>),
    Queue(Vec<CustomEvent>),
}

impl GlobalEventLoopProxyOrEventQueue {
    pub(crate) fn send_event(&mut self, event: CustomEvent) {
        match self {
            GlobalEventLoopProxyOrEventQueue::Proxy(proxy) => proxy.send_event(event).ok().unwrap(),
            GlobalEventLoopProxyOrEventQueue::Queue(queue) => {
                queue.push(event);
            }
        };
    }

    fn set_proxy(&mut self, proxy: winit::event_loop::EventLoopProxy<CustomEvent>) {
        match self {
            GlobalEventLoopProxyOrEventQueue::Proxy(_) => {}
            GlobalEventLoopProxyOrEventQueue::Queue(queue) => {
                std::mem::take(queue)
                    .into_iter()
                    .for_each(|event| proxy.send_event(event).ok().unwrap());
                *self = GlobalEventLoopProxyOrEventQueue::Proxy(proxy);
            }
        }
    }
}

impl Default for GlobalEventLoopProxyOrEventQueue {
    fn default() -> Self {
        Self::Queue(Vec::new())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) static GLOBAL_PROXY: once_cell::sync::OnceCell<
    std::sync::Mutex<GlobalEventLoopProxyOrEventQueue>,
> = once_cell::sync::OnceCell::new();

#[cfg(target_arch = "wasm32")]
thread_local! {
    pub(crate) static GLOBAL_PROXY: RefCell<Option<GlobalEventLoopProxyOrEventQueue>> = RefCell::new(None)
}

pub(crate) fn with_window_target<T>(callback: impl FnOnce(&dyn EventLoopInterface) -> T) -> T {
    if CURRENT_WINDOW_TARGET.is_set() {
        CURRENT_WINDOW_TARGET.with(|current_target| callback(current_target))
    } else {
        MAYBE_LOOP_INSTANCE.with(|loop_instance| {
            if loop_instance.borrow().is_none() {
                *loop_instance.borrow_mut() = Some(NotRunningEventLoop::new());
            }
            callback(loop_instance.borrow().as_ref().unwrap())
        })
    }
}

pub fn register_window(id: winit::window::WindowId, window: Rc<dyn WinitWindow>) {
    ALL_WINDOWS.with(|windows| {
        windows.borrow_mut().insert(id, Rc::downgrade(&window));
    })
}

pub fn unregister_window(id: winit::window::WindowId) {
    ALL_WINDOWS.with(|windows| {
        windows.borrow_mut().remove(&id);
    })
}

fn window_by_id(id: winit::window::WindowId) -> Option<Rc<dyn WinitWindow>> {
    ALL_WINDOWS.with(|windows| windows.borrow().get(&id).and_then(|weakref| weakref.upgrade()))
}

/// This enum captures run-time specific events that can be dispatched to the event loop in
/// addition to the winit events.
pub enum CustomEvent {
    /// Request for the event loop to wake up and redraw all windows. This is used on the
    /// web for example to request an animation frame.
    #[cfg(target_arch = "wasm32")]
    RedrawAllWindows,
    /// On wasm request_redraw doesn't wake the event loop, so we need to manually send an event
    /// so that the event loop can run
    #[cfg(target_arch = "wasm32")]
    WakeEventLoopWorkaround,
    UpdateWindowProperties(winit::window::WindowId),
    UserEvent(Box<dyn FnOnce() + Send>),
    /// Called from `GLWindow::hide` so that we can check if we should quit the event loop
    WindowHidden,
    Exit,
}

impl std::fmt::Debug for CustomEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(target_arch = "wasm32")]
            Self::RedrawAllWindows => write!(f, "RedrawAllWindows"),
            #[cfg(target_arch = "wasm32")]
            Self::WakeEventLoopWorkaround => write!(f, "WakeEventLoopWorkaround"),
            Self::UpdateWindowProperties(e) => write!(f, "UpdateWindowProperties({:?})", e),
            Self::UserEvent(_) => write!(f, "UserEvent"),
            Self::WindowHidden => write!(f, "WindowHidden"),
            Self::Exit => write!(f, "Exit"),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn redraw_all_windows() {
    let all_windows_weak =
        ALL_WINDOWS.with(|windows| windows.borrow().values().cloned().collect::<Vec<_>>());
    for window_weak in all_windows_weak {
        if let Some(window) = window_weak.upgrade() {
            window.request_redraw();
        }
    }
}

mod key_codes {
    macro_rules! winit_key_to_string_fn {
        ($($char:literal # $name:ident # $($_qt:ident)|* # $($winit:ident)|* ;)*) => {
            pub fn winit_key_to_string(virtual_keycode: winit::event::VirtualKeyCode) -> Option<i_slint_core::SharedString> {
                let char = match(virtual_keycode) {
                    $($(winit::event::VirtualKeyCode::$winit => $char,)*)*
                    _ => return None,
                };
                let mut buffer = [0; 6];
                Some(i_slint_core::SharedString::from(char.encode_utf8(&mut buffer) as &str))
            }
        };
    }
    i_slint_common::for_each_special_keys!(winit_key_to_string_fn);
}

fn process_window_event(
    window: Rc<dyn WinitWindow>,
    event: WindowEvent,
    cursor_pos: &mut Point,
    pressed: &mut bool,
) {
    fn key_event(
        event_type: KeyEventType,
        text: SharedString,
        modifiers: KeyboardModifiers,
    ) -> KeyEvent {
        let mut event = KeyEvent { event_type, text, modifiers };

        let tab = String::from(corelib::input::key_codes::Tab);

        // map Shift-Tab into (Shift) Backtab to have a similar behavior as Qt backend
        if event.text == tab && modifiers.shift {
            event.text = SharedString::from(String::from(corelib::input::key_codes::Backtab));
        }

        event
    }

    let runtime_window = window.runtime_window();
    match event {
        WindowEvent::Resized(size) => {
            window.resize_event(size);
        }
        WindowEvent::CloseRequested => {
            if runtime_window.request_close() {
                window.hide();
            }
        }
        WindowEvent::ReceivedCharacter(ch) => {
            // On Windows, X11 and Wayland sequences like Ctrl+C will send a ReceivedCharacter after the pressed keyboard input event,
            // with a control character. We choose not to forward those but try to use the current key code instead.
            //
            // We do not want to change the text to the value of the key press when that was a
            // control key itself: We already sent that event when handling the KeyboardInput.
            let text: Option<SharedString> = if ch.is_control() {
                window
                    .currently_pressed_key_code()
                    .take()
                    .and_then(winit_key_code_to_string)
                    .filter(|key_text| !key_text.starts_with(char::is_control))
            } else {
                Some(ch.to_string().into())
            };

            let text = match text {
                Some(text) => text,
                None => return,
            };

            let modifiers = window.current_keyboard_modifiers().get();

            let mut event = key_event(KeyEventType::KeyPressed, text, modifiers);

            runtime_window.clone().process_key_input(&event);
            event.event_type = KeyEventType::KeyReleased;
            runtime_window.process_key_input(&event);
        }
        WindowEvent::Focused(have_focus) => {
            let have_focus = have_focus || window.input_method_focused();
            // We don't render popups as separate windows yet, so treat
            // focus to be the same as being active.
            if have_focus != runtime_window.active() {
                runtime_window.set_active(have_focus);
                runtime_window.set_focus(have_focus);
            }
        }
        WindowEvent::KeyboardInput { ref input, .. } => {
            window.currently_pressed_key_code().set(match input.state {
                winit::event::ElementState::Pressed => input.virtual_keycode,
                _ => None,
            });
            if let Some(text) = input.virtual_keycode.and_then(key_codes::winit_key_to_string) {
                #[allow(unused_mut)]
                let mut modifiers = window.current_keyboard_modifiers().get();
                // On wasm, the WindowEvent::ModifiersChanged event is not received
                #[cfg(target_arch = "wasm32")]
                #[allow(deprecated)]
                {
                    modifiers.shift |= input.modifiers.shift();
                    modifiers.control |= input.modifiers.ctrl();
                    modifiers.meta |= input.modifiers.logo();
                    modifiers.alt |= input.modifiers.alt();
                }

                let event = key_event(
                    match input.state {
                        winit::event::ElementState::Pressed => KeyEventType::KeyPressed,
                        winit::event::ElementState::Released => KeyEventType::KeyReleased,
                    },
                    text,
                    modifiers,
                );
                runtime_window.process_key_input(&event);
            };
        }
        WindowEvent::ModifiersChanged(state) => {
            // To provide an easier cross-platform behavior, we map the command key to control
            // on macOS, and control to meta.
            #[cfg(target_os = "macos")]
            let (control, meta) = (state.logo(), state.ctrl());
            #[cfg(not(target_os = "macos"))]
            let (control, meta) = (state.ctrl(), state.logo());
            let modifiers =
                KeyboardModifiers { shift: state.shift(), alt: state.alt(), control, meta };
            window.current_keyboard_modifiers().set(modifiers);
        }
        WindowEvent::CursorMoved { position, .. } => {
            let position = position.to_logical(runtime_window.scale_factor() as f64);
            *cursor_pos = euclid::point2(position.x, position.y);
            runtime_window.process_mouse_input(MouseEvent::Moved { position: *cursor_pos });
        }
        WindowEvent::CursorLeft { .. } => {
            // On the html canvas, we don't get the mouse move or release event when outside the canvas. So we have no choice but canceling the event
            if cfg!(target_arch = "wasm32") || !*pressed {
                *pressed = false;
                runtime_window.process_mouse_input(MouseEvent::Exit);
            }
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let delta = match delta {
                winit::event::MouseScrollDelta::LineDelta(lx, ly) => {
                    euclid::point2(lx * 60., ly * 60.)
                }
                winit::event::MouseScrollDelta::PixelDelta(d) => {
                    let d = d.to_logical(runtime_window.scale_factor() as f64);
                    euclid::point2(d.x, d.y)
                }
            }
            .cast::<Coord>();
            runtime_window.process_mouse_input(MouseEvent::Wheel { position: *cursor_pos, delta });
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let button = match button {
                winit::event::MouseButton::Left => PointerEventButton::Left,
                winit::event::MouseButton::Right => PointerEventButton::Right,
                winit::event::MouseButton::Middle => PointerEventButton::Middle,
                winit::event::MouseButton::Other(_) => PointerEventButton::None,
            };
            let ev = match state {
                winit::event::ElementState::Pressed => {
                    *pressed = true;
                    MouseEvent::Pressed { position: *cursor_pos, button }
                }
                winit::event::ElementState::Released => {
                    *pressed = false;
                    MouseEvent::Released { position: *cursor_pos, button }
                }
            };
            runtime_window.process_mouse_input(ev);
        }
        WindowEvent::Touch(touch) => {
            let location = touch.location.to_logical(runtime_window.scale_factor() as f64);
            let position = euclid::point2(location.x, location.y);
            let ev = match touch.phase {
                winit::event::TouchPhase::Started => {
                    *pressed = true;
                    MouseEvent::Pressed { position, button: PointerEventButton::Left }
                }
                winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                    *pressed = false;
                    MouseEvent::Released { position, button: PointerEventButton::Left }
                }
                winit::event::TouchPhase::Moved => MouseEvent::Moved { position },
            };
            runtime_window.process_mouse_input(ev);
        }
        WindowEvent::ScaleFactorChanged { scale_factor, new_inner_size: size } => {
            if std::env::var("SLINT_SCALE_FACTOR").is_err() {
                let size = size.to_logical(scale_factor);
                runtime_window.set_window_item_geometry(size.width, size.height);
                runtime_window.set_scale_factor(scale_factor as f32);
            }
        }
        _ => {}
    }
}

/// Runs the event loop and renders the items in the provided `component` in its
/// own window.
#[allow(unused_mut)] // mut need changes for wasm
pub fn run(quit_behavior: i_slint_core::backend::EventLoopQuitBehavior) {
    use winit::event::Event;
    use winit::event_loop::{ControlFlow, EventLoopWindowTarget};

    let not_running_loop_instance = MAYBE_LOOP_INSTANCE.with(|loop_instance| {
        loop_instance.borrow_mut().take().unwrap_or_else(NotRunningEventLoop::new)
    });

    let event_loop_proxy = not_running_loop_instance.event_loop_proxy;
    #[cfg(not(target_arch = "wasm32"))]
    GLOBAL_PROXY.get_or_init(Default::default).lock().unwrap().set_proxy(event_loop_proxy.clone());
    #[cfg(target_arch = "wasm32")]
    GLOBAL_PROXY.with(|global_proxy| {
        global_proxy
            .borrow_mut()
            .get_or_insert_with(Default::default)
            .set_proxy(event_loop_proxy.clone())
    });

    let mut winit_loop = not_running_loop_instance.instance;
    let clipboard = not_running_loop_instance.clipboard;

    // Applying the property values of the `Window` item in .slint files to the underlying winit Window
    // may for example set the width but preserve the height of the window. Winit only knows the entire
    // size, so we need to query the existing size via `inner_size()`. The timing of when to call `inner_size()`
    // is important to avoid jitter as described in https://github.com/slint-ui/slint/issues/1269 . We want to
    // process pending resize events first. That is why this vector collects the window ids of windows that
    // we have scheduled for a sync between the `Window` item properties the the winit window, so that we
    // apply them at `MainEventsCleared` time.
    let mut windows_with_pending_property_updates = Vec::new();

    // last seen cursor position, (physical coordinate)
    let mut cursor_pos = Point::default();
    let mut pressed = false;
    let mut run_fn = move |event: Event<CustomEvent>,
                           event_loop_target: &EventLoopWindowTarget<CustomEvent>,
                           control_flow: &mut ControlFlow| {
        let running_instance = RunningEventLoop {
            event_loop_target,
            event_loop_proxy: &event_loop_proxy,
            clipboard: &clipboard,
        };
        CURRENT_WINDOW_TARGET.set(&running_instance, || {
            // MainEventsCleared and RedrawEventsCleared are passed after flush of the event loop
            // by winit. Make sure not to overwrite a previously set ControlFlow::Poll in that process.
            if !matches!(
                event,
                winit::event::Event::MainEventsCleared | winit::event::Event::RedrawEventsCleared
            ) {
                *control_flow = ControlFlow::Wait;
            }

            match event {
                winit::event::Event::WindowEvent { event, window_id } => {
                    if let Some(window) = window_by_id(window_id) {
                        process_window_event(window, event, &mut cursor_pos, &mut pressed);
                    };
                }

                winit::event::Event::RedrawRequested(id) => {
                    if let Some(window) = window_by_id(id) {
                        window.draw();
                    }
                }

                winit::event::Event::UserEvent(CustomEvent::UpdateWindowProperties(window_id)) => {
                    if let Err(insert_pos) =
                        windows_with_pending_property_updates.binary_search(&window_id)
                    {
                        windows_with_pending_property_updates.insert(insert_pos, window_id);
                    }
                }
                winit::event::Event::UserEvent(CustomEvent::WindowHidden) => match quit_behavior {
                    corelib::backend::EventLoopQuitBehavior::QuitOnLastWindowClosed => {
                        let window_count = ALL_WINDOWS.with(|windows| windows.borrow().len());
                        if window_count == 0 {
                            *control_flow = winit::event_loop::ControlFlow::Exit;
                        }
                    }
                    corelib::backend::EventLoopQuitBehavior::QuitOnlyExplicitly => {}
                },

                winit::event::Event::UserEvent(CustomEvent::Exit) => {
                    *control_flow = winit::event_loop::ControlFlow::Exit;
                }

                winit::event::Event::UserEvent(CustomEvent::UserEvent(user)) => {
                    user();
                }

                #[cfg(target_arch = "wasm32")]
                winit::event::Event::UserEvent(CustomEvent::RedrawAllWindows) => {
                    redraw_all_windows()
                }

                #[cfg(target_arch = "wasm32")]
                winit::event::Event::UserEvent(CustomEvent::WakeEventLoopWorkaround) => {
                    *control_flow = winit::event_loop::ControlFlow::Poll;
                }

                winit::event::Event::NewEvents(_) => {
                    corelib::timers::update_timers();
                }

                winit::event::Event::MainEventsCleared => {
                    for window in windows_with_pending_property_updates
                        .drain(..)
                        .flat_map(|window_id| window_by_id(window_id))
                    {
                        window.runtime_window().update_window_properties();
                    }
                }

                _ => (),
            }

            if *control_flow != winit::event_loop::ControlFlow::Exit
                && corelib::animations::CURRENT_ANIMATION_DRIVER
                    .with(|driver| driver.has_active_animations())
            {
                *control_flow = ControlFlow::Poll;
            }

            if *control_flow == winit::event_loop::ControlFlow::Wait {
                if let Some(next_timer) = corelib::timers::TimerList::next_timeout() {
                    *control_flow = winit::event_loop::ControlFlow::WaitUntil(next_timer.into());
                }
            }
        })
    };

    #[cfg(not(target_arch = "wasm32"))]
    {
        winit_loop.run_return(run_fn);

        *GLOBAL_PROXY.get_or_init(Default::default).lock().unwrap() = Default::default();
    }

    #[cfg(target_arch = "wasm32")]
    {
        winit_loop.run(run_fn)
    }
}

// This function is called when we receive a control character via WindowEvent::ReceivedCharacter and
// instead want to use the last virtual key code. That happens when for example pressing Ctrl+some_key
// on Windows/X11/Wayland. This function may be missing mappings, it's trying to cover what we may be
// getting when we're getting control character sequences.
fn winit_key_code_to_string(virtual_keycode: winit::event::VirtualKeyCode) -> Option<SharedString> {
    use winit::event::VirtualKeyCode;
    Some(
        match virtual_keycode {
            VirtualKeyCode::Key1 => "1",
            VirtualKeyCode::Key2 => "2",
            VirtualKeyCode::Key3 => "3",
            VirtualKeyCode::Key4 => "4",
            VirtualKeyCode::Key5 => "5",
            VirtualKeyCode::Key6 => "6",
            VirtualKeyCode::Key7 => "7",
            VirtualKeyCode::Key8 => "8",
            VirtualKeyCode::Key9 => "9",
            VirtualKeyCode::Key0 => "0",
            VirtualKeyCode::A => "a",
            VirtualKeyCode::B => "b",
            VirtualKeyCode::C => "c",
            VirtualKeyCode::D => "d",
            VirtualKeyCode::E => "e",
            VirtualKeyCode::F => "f",
            VirtualKeyCode::G => "g",
            VirtualKeyCode::H => "h",
            VirtualKeyCode::I => "i",
            VirtualKeyCode::J => "j",
            VirtualKeyCode::K => "k",
            VirtualKeyCode::L => "l",
            VirtualKeyCode::M => "m",
            VirtualKeyCode::N => "n",
            VirtualKeyCode::O => "o",
            VirtualKeyCode::P => "p",
            VirtualKeyCode::Q => "q",
            VirtualKeyCode::R => "r",
            VirtualKeyCode::S => "s",
            VirtualKeyCode::T => "t",
            VirtualKeyCode::U => "u",
            VirtualKeyCode::V => "v",
            VirtualKeyCode::W => "w",
            VirtualKeyCode::X => "x",
            VirtualKeyCode::Y => "y",
            VirtualKeyCode::Z => "z",
            VirtualKeyCode::Space => " ",
            VirtualKeyCode::Caret => "^",
            VirtualKeyCode::Apostrophe => "'",
            VirtualKeyCode::Asterisk => "*",
            VirtualKeyCode::Backslash => "\\",
            VirtualKeyCode::Colon => ":",
            VirtualKeyCode::Comma => ",",
            VirtualKeyCode::Equals => "=",
            VirtualKeyCode::Grave => "`",
            VirtualKeyCode::Minus => "-",
            VirtualKeyCode::Period => ".",
            VirtualKeyCode::Plus => "+",
            VirtualKeyCode::Semicolon => ";",
            VirtualKeyCode::Slash => "/",
            VirtualKeyCode::Tab => "\t",
            _ => return None,
        }
        .into(),
    )
}

fn create_clipboard<T>(
    _event_loop: &winit::event_loop::EventLoopWindowTarget<T>,
) -> Box<dyn copypasta::ClipboardProvider> {
    #[allow(unused_mut)]
    let mut clipboard: Option<Box<dyn copypasta::ClipboardProvider>> = None;

    cfg_if::cfg_if! {
        if #[cfg(all(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ), feature = "wayland", not(feature = "x11")))] {
            type DefaultClipboard = copypasta::nop_clipboard::NopClipboardContext;
        } else {
            type DefaultClipboard = copypasta::ClipboardContext;
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    if cfg!(feature = "wayland") {
        use winit::platform::unix::EventLoopWindowTargetExtUnix;
        #[cfg(feature = "wayland")]
        if let Some(wayland_display) = _event_loop.wayland_display() {
            clipboard = unsafe {
                Some(Box::new(
                    copypasta::wayland_clipboard::create_clipboards_from_external(wayland_display)
                        .1,
                ))
            };
        }
    }

    clipboard.unwrap_or_else(|| Box::new(DefaultClipboard::new().unwrap()))
}