//! The window: winit's events, turned into input for the stand-in, the device screen's
//! touch targets and the dev keyboard.

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, KeyEvent, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window, WindowId};

use super::keymap::{self, KeyContext, Shortcut};
use super::{App, PointerOn};
use crate::render::{self, Gfx};
use crate::screens::ScreenKind;
use crate::screens::browse::ROW_H;
#[cfg(feature = "standin")]
use crate::standin;
use waveform::display::Mode;

impl App {
    /// The pointer went down at window pixels (`x`, `y`); `alt` = right button or shift.
    fn pointer_down(&mut self, x: f64, y: f64, alt: bool) {
        #[cfg(feature = "standin")]
        if self.standin.is_some()
            && let Some(gfx) = &self.gfx
            && !gfx.on_device(x, y)
            && let Some((sx, sy)) = gfx.to_shell(x, y)
        {
            self.pointer_on = PointerOn::Shell;
            let alt = alt || self.modifiers.shift_key();
            let events = self
                .standin
                .as_mut()
                .map(|s| s.press(sx, sy, alt))
                .unwrap_or_default();
            for e in events {
                self.control(e);
            }
            return;
        }
        let _ = alt;
        self.pointer_on = PointerOn::Device;
        self.press(x, y);
    }

    fn pointer_move(&mut self, x: f64, y: f64) {
        #[cfg(feature = "standin")]
        if self.pointer_on == PointerOn::Shell {
            let point = self.gfx.as_ref().and_then(|g| g.to_shell(x, y));
            let events = match (point, self.standin.as_mut()) {
                (Some((sx, sy)), Some(s)) => s.drag(sx, sy),
                _ => Vec::new(),
            };
            for e in events {
                self.control(e);
            }
            return;
        }
        self.drag(x, y);
    }

    fn pointer_up(&mut self) {
        #[cfg(feature = "standin")]
        if self.pointer_on == PointerOn::Shell {
            let events = self
                .standin
                .as_mut()
                .map(|s| s.release())
                .unwrap_or_default();
            for e in events {
                self.control(e);
            }
            self.pointer_on = PointerOn::Device;
            return;
        }
        self.release();
    }

    /// A touch on the device screen at window pixels (`x`, `y`).
    fn press(&mut self, x: f64, y: f64) {
        let Some(gfx) = &self.gfx else { return };
        let at = gfx.to_design(x, y);
        let Some(hit) = self.hits.find(at.0, at.1) else {
            return;
        };
        let (screens, mut cx) = self.split();
        screens.tap(hit, at, &mut cx);
    }

    fn drag(&mut self, x: f64, y: f64) {
        let Some(gfx) = &self.gfx else { return };
        let at = gfx.to_design(x, y);
        let (screens, mut cx) = self.split();
        screens.drag(at, &mut cx);
    }

    /// The finger or button came up.
    fn release(&mut self) {
        let (screens, mut cx) = self.split();
        screens.release(&mut cx);
    }

    /// The wheel turned `rows` (positive = down) at window pixels (`x`, `y`).
    fn wheel(&mut self, x: f64, y: f64, rows: i32) {
        #[cfg(feature = "standin")]
        if self.standin.is_some()
            && let Some(gfx) = &self.gfx
            && !gfx.on_device(x, y)
        {
            let turn = match (gfx.to_shell(x, y), self.standin.as_mut()) {
                (Some((sx, sy)), Some(s)) => s.wheel(sx, sy, rows),
                _ => None,
            };
            if let Some(e) = turn {
                self.control(e);
            }
            return;
        }
        let _ = (x, y);
        let (screens, mut cx) = self.split();
        screens.wheel(rows, &mut cx);
    }

    /// A key of the dev keyboard. Text fields (naming, the search) take the keys first; the
    /// rest is the keymap's.
    fn key(&mut self, event_loop: &ActiveEventLoop, key: &KeyEvent) {
        if key.repeat {
            return;
        }
        let key = keymap::KeyPress::from(key);
        let screen = self.screens.screen;
        let browsing = screen == ScreenKind::Browse;
        if browsing {
            let (screens, mut cx) = self.split();
            if screens.browse.key(&key, &mut cx) {
                return;
            }
        }
        if screen == ScreenKind::Settings {
            let (screens, mut cx) = self.split();
            if screens.settings.key(&key, &mut cx) {
                return;
            }
        }
        let ctx = KeyContext {
            // The arrows and Enter move through the settings as through a list.
            browsing: browsing || screen == ScreenKind::Settings,
            info: screen == ScreenKind::Info,
            shift: self.modifiers.shift_key(),
        };
        let Some(shortcut) = keymap::shortcut(&key, ctx) else {
            return;
        };
        match shortcut {
            Shortcut::Control(e) => self.control(e),
            Shortcut::Deck(d) => self.send(d.command(self.deck.sample_rate)),
            Shortcut::Nav(n) => self.nav(n),
            Shortcut::Perform => self.screens.toggle_perform(),
            Shortcut::ToggleBrowse => {
                let (screens, cx) = self.split();
                screens.toggle_browse(&cx);
            }
            Shortcut::Search => {
                let (screens, mut cx) = self.split();
                screens.search(&mut cx);
            }
            Shortcut::Info => self.screens.toggle_info(),
            Shortcut::Debug => self.debug = !self.debug,
            Shortcut::Memory => eprintln!("{}", self.player.memory()),
            Shortcut::WaveMode => {
                self.wave_settings.mode = match self.wave_settings.mode {
                    Mode::ThreeBand => Mode::Rgb,
                    Mode::Rgb => Mode::Single,
                    Mode::Single => Mode::ThreeBand,
                };
            }
            Shortcut::Fullscreen => {
                if let Some(g) = &self.gfx {
                    let w = g.window();
                    w.set_fullscreen(match w.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(None)),
                    });
                }
            }
            Shortcut::Quit => event_loop.exit(),
        }
    }

    /// The window's logical size: the device screen, or the stand-in around it.
    fn window_size(&self) -> LogicalSize<f32> {
        #[cfg(feature = "standin")]
        if self.standin.is_some() {
            return LogicalSize::new(standin::SIZE.0, standin::SIZE.1);
        }
        LogicalSize::new(render::DESIGN_W, render::DESIGN_H)
    }

    /// One window event: input, a redraw, a resize.
    fn handle(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gfx {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if self.frame() {
                    return event_loop.exit();
                }
                if let Some(g) = &self.gfx {
                    g.window().request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::MouseWheel { delta, .. } => {
                // Rows: a notch is one; a trackpad scrolls a row per row height of travel.
                self.wheel -= match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => p.y / f64::from(ROW_H),
                };
                let rows = self.wheel.trunc();
                if rows != 0.0 {
                    self.wheel -= rows;
                    self.wheel(self.cursor.0, self.cursor.1, rows as i32);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(event_loop, &event),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.pointer_move(position.x, position.y);
            }
            WindowEvent::MouseInput {
                state,
                button: button @ (MouseButton::Left | MouseButton::Right),
                ..
            } => match state {
                ElementState::Pressed => {
                    let alt = button == MouseButton::Right;
                    self.pointer_down(self.cursor.0, self.cursor.1, alt)
                }
                ElementState::Released => self.pointer_up(),
            },
            WindowEvent::Touch(Touch {
                phase, location, ..
            }) => match phase {
                TouchPhase::Started => self.pointer_down(location.x, location.y, false),
                TouchPhase::Moved => self.pointer_move(location.x, location.y),
                TouchPhase::Ended | TouchPhase::Cancelled => self.pointer_up(),
            },
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("dj-ui")
            .with_inner_size(self.window_size())
            .with_resizable(true);
        if self.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("window: {e}"));
                return event_loop.exit();
            }
        };
        match Gfx::new(window, event_loop) {
            Ok(g) => {
                eprintln!(
                    "{} ({:?}), {:?}, frame latency 1",
                    g.adapter_info.name, g.adapter_info.backend, g.present_mode
                );
                g.window().request_redraw();
                self.gfx = Some(g);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.handle(event_loop, event);
        self.apply_effects();
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        for line in self.player.poll() {
            eprintln!("{line}");
        }
    }
}
