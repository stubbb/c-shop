//! Headless input simulation.
//!
//! The rest of the test suite calls the application's methods directly, which
//! never exercises the part that actually routes a click: egui's hit-testing
//! across panels, areas and overlapping interaction rects. A regression
//! reached the user through exactly that gap — a title-bar drag handle
//! registered on top of its own menus, leaving them and the close button dead
//! — so this drives the real [`CShopApp::update`] with synthetic pointer
//! events.
//!
//! Needs a GPU, and [`Harness::new`] returns `None` without one so the tests
//! skip rather than fail.

use cshop_core::color::Rgba8;
use cshop_gpu::context::GpuContext;
use crate::commands::WindowCommand;
use crate::CShopApp;

/// One step of a scripted interaction. Each is one frame.
#[derive(Clone, Copy)]
pub enum Step {
    Move(f32, f32),
    Press(f32, f32),
    Release(f32, f32),
    /// Let the interface settle, so animations and deferred work finish.
    Idle,
    /// Hold the modifiers and press the key.
    KeyDown(egui::Key, egui::Modifiers),
    /// Release the key and let the modifiers go.
    KeyUp(egui::Key),
}

pub struct Harness {
    pub ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    gpu: GpuContext,
    pub app: CShopApp,
    size: (u32, u32),
    frame: u32,
    /// Everything the interface asked the window to do, in order.
    pub window_commands: Vec<WindowCommand>,
}

impl Harness {
    /// `None` when there is no usable GPU.
    pub fn new(size: (u32, u32)) -> Option<Harness> {
        let gpu = GpuContext::headless()
            .inspect_err(|e| eprintln!("skipping input tests: {e}"))
            .ok()?;
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            egui_wgpu::RendererOptions::default(),
        );
        let app = CShopApp::new(gpu.clone());
        Some(Harness { ctx, renderer, gpu, app, size, frame: 0, window_commands: Vec::new() })
    }

    pub fn run(&mut self, steps: &[Step]) {
        for step in steps {
            let events = match *step {
                Step::Move(x, y) => vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                Step::Press(x, y) => vec![
                    egui::Event::PointerMoved(egui::pos2(x, y)),
                    egui::Event::PointerButton {
                        pos: egui::pos2(x, y),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
                Step::Release(x, y) => vec![egui::Event::PointerButton {
                    pos: egui::pos2(x, y),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                Step::Idle => Vec::new(),
                // `InputState::modifiers` is whatever the last
                // `ModifiersChanged` of the frame left behind, so pressing and
                // releasing in one frame would report no modifiers at all.
                // Press and release therefore take a frame each.
                Step::KeyDown(key, modifiers) => vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                Step::KeyUp(key) => vec![
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::ModifiersChanged(egui::Modifiers::default()),
                ],
            };
            self.frame(events);
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(self.size.0 as f32, self.size.1 as f32),
            )),
            time: Some(self.frame as f64 / 60.0),
            events,
            ..Default::default()
        };
        self.frame += 1;

        let app = &mut self.app;
        let renderer = &mut self.renderer;
        let mut output = self.ctx.run_ui(raw_input, |ui| app.update(ui, renderer));
        // Tessellating exercises the same paths a real frame would.
        let _ = self.ctx.tessellate(output.shapes, 1.0);

        // egui hands out font and image deltas each frame and panics if they
        // are dropped unapplied, so consume them exactly as the renderer does.
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        // `TexturesDelta` panics on drop unless it is emptied, which is how
        // egui catches a renderer that silently ignores its uploads.
        output.textures_delta.clear();

        // Mirror what the event loop does with queued window commands, so a
        // test can see them and `quit` behaves as it would in the real app.
        for command in std::mem::take(&mut self.app.window_commands) {
            if command == WindowCommand::Close {
                self.app.quit = true;
            }
            self.window_commands.push(command);
        }
    }

    /// A drag from one point to another, with intermediate moves.
    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32), steps: usize) {
        let mut script = vec![Step::Move(from.0, from.1), Step::Idle, Step::Press(from.0, from.1)];
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            script.push(Step::Move(
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
            ));
        }
        script.push(Step::Release(to.0, to.1));
        script.push(Step::Idle);
        self.run(&script);
    }

    /// Run a number of empty frames, so animations and deferred work finish.
    pub fn settle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame(Vec::new());
        }
    }

    /// A right-click, which is what opens the context menus.
    pub fn secondary_click(&mut self, at: (f32, f32)) {
        let pos = egui::pos2(at.0, at.1);
        // The pointer has to settle for a frame first: egui hit-tests using
        // the previous frame's geometry.
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        self.frame(
            vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: Default::default(),
            }]);
        self.frame(
            vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: Default::default(),
            }]);
        self.settle(2);
    }

    /// Where the Layers panel's New Layer button currently sits.
    ///
    /// Derived from the window size rather than hard-coded, so the tests
    /// survive a change of panel width.
    pub fn new_layer_button(&self) -> (f32, f32) {
        // Third button in the footer row, which is pinned to the bottom of the
        // Layers panel just above the status bar.
        let dock_left = self.size.0 as f32 - 280.0;
        let x = dock_left + 8.0 + 2.0 * 25.0 + 11.0;
        let y = self.size.1 as f32 - 24.0 - 23.0;
        (x, y)
    }

    /// Press and release one chord, then let it take effect.
    pub fn press(&mut self, chord: crate::shortcuts::Chord) {
        let modifiers = egui::Modifiers {
            alt: chord.alt,
            ctrl: chord.ctrl,
            shift: chord.shift,
            mac_cmd: false,
            command: chord.ctrl,
        };
        self.run(&[
            Step::KeyDown(chord.key, modifiers),
            Step::KeyUp(chord.key),
            Step::Idle,
            Step::Idle,
        ]);
    }

    /// Centre of a widget, as egui last laid it out.
    ///
    /// Asking egui beats recomputing the layout: if the widget moved, the
    /// test follows it, and if it was never registered the test says so.
    pub fn widget_center(&self, id: egui::Id) -> Option<(f32, f32)> {
        let rect = self.ctx.read_response(id)?.rect;
        Some((rect.center().x, rect.center().y))
    }

    /// A click: move, settle, press, release.
    pub fn click(&mut self, at: (f32, f32)) {
        self.run(&[
            Step::Move(at.0, at.1),
            Step::Idle,
            Step::Press(at.0, at.1),
            Step::Release(at.0, at.1),
            Step::Idle,
            Step::Idle,
        ]);
    }

    /// Colour of the active layer at a document pixel.
    pub fn active_pixel(&self, x: i32, y: i32) -> Option<Rgba8> {
        let view = self.app.doc()?;
        let id = view.doc.active?;
        Some(view.doc.tree.get(id)?.pixels()?.get(x, y))
    }

    /// Where a document point currently sits on screen.
    pub fn doc_to_screen(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let view = self.app.doc()?;
        let p = view.doc_to_screen(self.app.canvas_viewport, egui::vec2(x, y));
        Some((p.x, p.y))
    }
}

