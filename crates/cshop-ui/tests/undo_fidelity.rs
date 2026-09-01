//! Does undo actually put the document back?
//!
//! Every command claims to be reversible, and a command that is *nearly*
//! reversible is the worst kind: it looks right at the moment you press
//! Ctrl+Z, and the drift only shows up after ten of them. So these do not
//! check that undo "worked" — they take a fingerprint of the entire document
//! before each edit and require it back, bit for bit, afterwards.
//!
//! The fingerprint covers everything a user could notice: the tree's shape and
//! order, every layer's pixels, mask, offset, opacity, blend mode, visibility,
//! locks, effects and attached filters, the selection with its coverage, the
//! saved channels, the guides, the smart-object sources, and the document's
//! own size and depth. If undo leaves any of that different, these fail and
//! say which layer and which field.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

// ---------------------------------------------------------------------------
// Fingerprinting
// ---------------------------------------------------------------------------

/// One number standing for the whole document.
///
/// A hash rather than a copy: the comparison is per-step over long chains, and
/// keeping a hundred whole documents to compare against would measure the
/// machine's memory rather than the undo stack.
fn fingerprint(doc: &Document) -> u64 {
    let mut h = Hasher::new();
    h.u64(doc.width as u64);
    h.u64(doc.height as u64);
    h.str(&doc.name);
    h.u64(doc.tree.len() as u64);

    for id in doc.tree.iter_all() {
        let Some(l) = doc.tree.get(id) else { continue };
        h.u64(l.id.0);
        h.str(&l.name);
        h.u64(l.parent.map_or(u64::MAX, |p| p.0));
        h.u64(l.visible as u64);
        h.f32(l.opacity);
        h.f32(l.fill_opacity);
        h.u64(l.blend_mode as u64);
        h.u64(l.clipping as u64);
        h.u64(l.is_background as u64);
        h.i32(l.offset.0);
        h.i32(l.offset.1);
        h.u64(l.locks.any() as u64);
        h.str(&format!("{:?}", l.effects.active_names()));
        h.u64(l.filters.slots.len() as u64);
        for slot in &l.filters.slots {
            h.str(&format!("{:?}", slot.filter));
            h.u64(slot.enabled as u64);
            h.f32(slot.opacity);
        }
        // The kind, and whatever pixels it currently shows.
        h.str(match &l.kind {
            LayerKind::Raster(_) => "raster",
            LayerKind::Group { .. } => "group",
            LayerKind::Text(_) => "text",
            LayerKind::Shape(_) => "shape",
            LayerKind::Adjustment(_) => "adjustment",
            LayerKind::Fill(_) => "fill",
            LayerKind::Smart(_) => "smart",
        });
        if let LayerKind::Smart(s) = &l.kind {
            h.u64(s.source().0 as u64);
        }
        match l.pixels() {
            Some(px) => {
                h.u64(px.width() as u64);
                h.u64(px.height() as u64);
                h.bytes(px.as_bytes());
            }
            None => h.str("no-pixels"),
        }
        match &l.mask {
            Some(m) => {
                h.i32(m.offset.0);
                h.i32(m.offset.1);
                h.u64(m.enabled as u64);
                h.u64(m.linked as u64);
                h.bytes(m.data.as_bytes());
            }
            None => h.str("no-mask"),
        }
    }

    match &doc.selection {
        Some(s) => {
            let b = s.bounds();
            h.i32(b.x0);
            h.i32(b.y0);
            h.i32(b.x1);
            h.i32(b.y1);
            h.bytes(s.to_mask().as_bytes());
        }
        None => h.str("no-selection"),
    }

    h.u64(doc.channels.len() as u64);
    for c in &doc.channels {
        h.str(&c.name);
        h.u64(c.visible as u64);
        h.bytes(c.data.as_bytes());
    }
    h.u64(doc.guides.len() as u64);
    for g in &doc.guides {
        h.u64(g.vertical as u64);
        h.f32(g.at);
    }
    h.u64(doc.sources.len() as u64);
    for (id, s) in doc.sources.iter() {
        h.u64(id.0 as u64);
        h.bytes(s.pixels.as_bytes());
    }
    h.finish()
}

/// FNV-1a, which is plenty for telling two documents apart and is a dozen
/// lines rather than a dependency.
struct Hasher(u64);

impl Hasher {
    fn new() -> Hasher {
        Hasher(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for byte in b {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.bytes(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.bytes(&v.to_bits().to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn app(w: u32, h: u32) -> Option<CShopApp> {
    let gpu = GpuContext::headless()
        .inspect_err(|e| eprintln!("skipping undo tests: {e}"))
        .ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", w, h, Background::White));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(photo(w, h));
    view.invalidate();
    Some(app)
}

/// Something with structure in it, so a filter or a stroke has an effect that
/// a fingerprint can see.
fn photo(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    let mut s: u32 = 0x1234_5678;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((s >> 22) & 0x1f) as u8;
            let a = if (x / 11 + y / 13) % 2 == 0 { 60u8 } else { 0 };
            px.set(x, y, Rgba8::opaque(90 + n + a, 130 - n, 200 - a));
        }
    }
    px
}


/// The same, with long work on worker threads as the window has it.
fn worker_app(side: u32) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu).with_workers();
    app.open_document(Document::new("t", side, side, Background::Transparent));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(photo(side, side));
    view.invalidate();
    Some(app)
}

/// Collect every finished job, or give up.
fn settle(app: &mut CShopApp) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        app.collect_jobs();
        if !app.jobs.any() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    panic!("a job never finished");
}

fn print(app: &CShopApp) -> u64 {
    fingerprint(&app.doc().unwrap().doc)
}

fn cursor(app: &CShopApp) -> usize {
    app.doc().unwrap().history.cursor()
}

fn labels(app: &CShopApp) -> Vec<String> {
    app.doc().unwrap().history.labels()
}

/// One named step of a chain: what to call it, and what to do.
type Step = (&'static str, fn(&mut CShopApp));

/// Walk a chain of edits, remembering the document after each, then undo the
/// whole way back and redo the whole way forward, requiring every state to
/// come back exactly.
///
/// Returns the labels of the steps that actually recorded history, so a caller
/// can assert on what was recorded as well as on what came back.
fn round_trip(app: &mut CShopApp, steps: &[Step]) -> Vec<String> {
    // Cursor as well as fingerprint, because a gesture is not always one
    // entry: Flatten records a deletion per layer it absorbs, Merge Down
    // records two. Undoing back to a state means undoing to its *cursor*, not
    // pressing Ctrl+Z once.
    let mut states = vec![(app.doc().unwrap().history.cursor(), print(app))];
    let mut applied: Vec<&str> = Vec::new();

    for (name, step) in steps {
        let before = cursor(app);
        step(app);
        if cursor(app) > before {
            states.push((cursor(app), print(app)));
            applied.push(name);
        } else {
            assert_eq!(
                print(app),
                states.last().unwrap().1,
                "{name} recorded no history but changed the document — \
                 that edit can never be undone"
            );
        }
    }

    // Back, a state at a time.
    for i in (0..states.len() - 1).rev() {
        let (want_cursor, want) = states[i];
        while cursor(app) > want_cursor {
            app.dispatch(Action::Undo);
        }
        assert_eq!(
            print(app),
            want,
            "undoing back past {:?} did not restore the document to state {i}",
            applied.get(i).copied().unwrap_or("?")
        );
    }
    assert_eq!(cursor(app), states[0].0, "the history should be back at the start");

    // And forward again.
    for (i, &(want_cursor, want)) in states.iter().enumerate().skip(1) {
        while cursor(app) < want_cursor {
            app.dispatch(Action::Redo);
        }
        assert_eq!(
            print(app),
            want,
            "redoing forward to {:?} did not reproduce state {i}",
            applied.get(i - 1).copied().unwrap_or("?")
        );
    }
    applied.into_iter().map(String::from).collect()
}

// ---------------------------------------------------------------------------
// Long chains of simple tools
// ---------------------------------------------------------------------------

/// Sixty strokes with six tools, undone one at a time.
///
/// The point is the length. A single stroke undoes correctly in the existing
/// tests; what this asks is whether the fortieth does, after the stack has
/// been through every kind of entry the painting tools make.
#[test]
fn a_long_run_of_strokes_undoes_to_exactly_where_it_started() {
    use cshop_ui::tools::Tool;
    let Some(mut app) = app(200, 150) else { return };
    let start = print(&app);

    let tools = [
        Tool::Brush,
        Tool::Eraser,
        Tool::Pencil,
        Tool::Dodge,
        Tool::Blur,
        Tool::Smudge,
    ];
    let mut states = vec![start];
    for i in 0..60 {
        app.tool = tools[i % tools.len()];
        app.brush.size = 6.0 + (i % 5) as f32 * 3.0;
        app.foreground = Rgba8::opaque((i * 37 % 255) as u8, 90, (i * 11 % 255) as u8);
        let y = 12.0 + (i % 10) as f32 * 13.0;
        let x = 10.0 + (i % 7) as f32 * 20.0;
        app.begin_stroke(Vec2::new(x, y), PaintMode::Paint);
        app.continue_stroke(Vec2::new(x + 24.0, y + 7.0));
        app.continue_stroke(Vec2::new(x + 40.0, y - 4.0));
        app.end_stroke();
        states.push(print(&app));
    }

    // Every stroke has to have left a distinct document, or the test is
    // undoing nothing and would pass on a broken stack.
    let distinct: std::collections::HashSet<u64> = states.iter().copied().collect();
    assert!(
        distinct.len() > 55,
        "the strokes should have changed the picture each time, got {} distinct states of {}",
        distinct.len(),
        states.len()
    );

    for (i, want) in states.iter().enumerate().rev().skip(1) {
        app.dispatch(Action::Undo);
        assert_eq!(print(&app), *want, "the document drifted undoing back to step {i}");
    }
    assert_eq!(print(&app), start);
    assert_eq!(cursor(&app), 0);

    for (i, want) in states.iter().enumerate().skip(1) {
        app.dispatch(Action::Redo);
        assert_eq!(print(&app), *want, "the document drifted redoing forward to step {i}");
    }
}

/// The tools that are not strokes: the bucket, the gradient, selections,
/// transforms, layer operations and the clipboard, in one chain.
#[test]
fn a_mixed_chain_of_ordinary_edits_undoes_exactly() {
    use cshop_core::selection::{Rectf, Selection};
    use cshop_ui::tools::Tool;
    let Some(mut app) = app(160, 120) else { return };

    let steps: &[Step] = &[
        ("new layer", |a| a.dispatch(Action::NewLayer)),
        ("fill", |a| {
            a.foreground = Rgba8::opaque(200, 40, 40);
            a.dispatch(Action::FillSwatch { background: false, preserve_transparency: false });
        }),
        ("select", |a| {
            let (w, h) = a.doc().map(|v| (v.doc.width, v.doc.height)).unwrap();
            let sel = Selection::from_rect(
                w,
                h,
                Rectf::from_points(Vec2::new(20.0, 20.0), Vec2::new(90.0, 80.0)),
                true,
            );
            a.dispatch(Action::SetSelection(Box::new(sel), "Select"));
        }),
        ("fill selection", |a| {
            a.foreground = Rgba8::opaque(20, 200, 90);
            a.dispatch(Action::FillSwatch { background: false, preserve_transparency: false });
        }),
        ("bucket", |a| {
            a.tool = Tool::PaintBucket;
            a.foreground = Rgba8::opaque(10, 30, 220);
            a.begin_stroke(Vec2::new(40.0, 40.0), PaintMode::Paint);
            a.end_stroke();
        }),
        ("rename", |a| {
            let id = a.doc().unwrap().doc.active.unwrap();
            a.dispatch(Action::SetLayerProperty(id, cshop_core::history::LayerProperty::Name("renamed".into())));
        }),
        ("opacity", |a| {
            let id = a.doc().unwrap().doc.active.unwrap();
            a.dispatch(Action::SetLayerProperty(id, cshop_core::history::LayerProperty::Opacity(0.42)));
        }),
        ("blend mode", |a| {
            let id = a.doc().unwrap().doc.active.unwrap();
            a.dispatch(Action::SetLayerProperty(id, cshop_core::history::LayerProperty::Blend(cshop_core::blend::BlendMode::Multiply)));
        }),
        ("duplicate", |a| a.dispatch(Action::DuplicateLayer)),
        ("move", |a| a.dispatch(Action::NudgeLayer(7, -3))),
        ("add mask", |a| a.dispatch(Action::AddLayerMask { hide_all: false })),
        ("deselect", |a| a.dispatch(Action::Deselect)),
        ("merge down", |a| a.dispatch(Action::MergeDown)),
        ("resize canvas", |a| {
            a.dispatch(Action::ResizeCanvas {
                width: 200,
                height: 140,
                anchor: cshop_ui::commands::Anchor::Center,
            })
        }),
    ];

    let applied = round_trip(&mut app, steps);
    assert!(
        applied.len() >= 12,
        "most of the chain should have recorded history, got {applied:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Every filter, applied and undone.
///
/// One chain rather than one document each, so each filter is undone from a
/// stack that already has every filter before it on it.
#[test]
fn every_filter_undoes_exactly() {
    use cshop_core::filters::Filter;
    let Some(mut app) = app(90, 70) else { return };

    let mut states = vec![print(&app)];
    let mut names = vec![String::from("origin")];
    for filter in Filter::examples() {
        let before = cursor(&app);
        let name = filter.name().to_string();
        app.dispatch(Action::ApplyFilter(Box::new(filter)));
        assert!(
            cursor(&app) > before,
            "{name} recorded no history — it cannot be undone"
        );
        states.push(print(&app));
        names.push(name);
    }

    for i in (1..states.len()).rev() {
        app.dispatch(Action::Undo);
        assert_eq!(
            print(&app),
            states[i - 1],
            "undoing {} left the document different from before it",
            names[i]
        );
    }
    for i in 1..states.len() {
        app.dispatch(Action::Redo);
        assert_eq!(print(&app), states[i], "redoing {} did not reproduce it", names[i]);
    }
}

/// A filter inside a selection touches only part of a layer, and its undo has
/// to put back only that part — including the feathered edge, where the filter
/// was blended rather than applied.
#[test]
fn a_filter_through_a_feathered_selection_undoes_exactly() {
    use cshop_core::filters::Filter;
    use cshop_core::selection::{Rectf, Selection};
    let Some(mut app) = app(120, 90) else { return };
    {
        let view = app.doc_mut().unwrap();
        let mut sel = Selection::from_rect(
            120,
            90,
            Rectf::from_points(Vec2::new(30.0, 20.0), Vec2::new(90.0, 70.0)),
            true,
        );
        sel.feather(6.0);
        view.doc.selection = Some(sel);
    }
    let before = print(&app);
    app.dispatch(Action::ApplyFilter(Box::new(Filter::GaussianBlur { radius: 8.0 })));
    assert_ne!(print(&app), before, "the filter should have changed something");
    app.dispatch(Action::Undo);
    assert_eq!(print(&app), before, "undoing a filter through a soft selection left a trace");
}

/// Attached filters are settings rather than pixels, so adding, editing and
/// removing them has to undo as settings.
#[test]
fn smart_filters_undo_as_settings() {
    use cshop_core::filters::Filter;
    let Some(mut app) = app(80, 60) else { return };

    let steps: &[Step] = &[
        ("attach blur", |a| {
            a.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 4.0 })))
        }),
        ("attach sharpen", |a| {
            a.dispatch(Action::AttachFilter(Box::new(Filter::Sharpen { amount: 1.0 })))
        }),
        ("change the first", |a| {
            a.dispatch(Action::ReplaceAttachedFilter(
                0,
                Box::new(Filter::GaussianBlur { radius: 12.0 }),
            ))
        }),
        ("half opacity", |a| a.dispatch(Action::SetAttachedFilterOpacity(0, 0.5))),
        ("switch one off", |a| a.dispatch(Action::ToggleAttachedFilter(1))),
        ("remove one", |a| a.dispatch(Action::RemoveAttachedFilter(0))),
        ("apply the stack", |a| a.dispatch(Action::ApplyAttachedFilters)),
    ];
    let applied = round_trip(&mut app, steps);
    assert!(applied.len() >= 6, "most of these should record history: {applied:?}");
}

// ---------------------------------------------------------------------------
// The history stack itself
// ---------------------------------------------------------------------------

/// Clicking about in the History panel jumps to arbitrary states. Each one has
/// to be the state that was actually there.
#[test]
fn jumping_around_the_history_panel_lands_on_the_right_states() {
    use cshop_core::filters::Filter;
    let Some(mut app) = app(100, 80) else { return };
    let mut states = vec![print(&app)];
    for r in [2.0f32, 5.0, 9.0, 3.0, 7.0, 4.0] {
        app.dispatch(Action::ApplyFilter(Box::new(Filter::GaussianBlur { radius: r })));
        states.push(print(&app));
    }

    // Out of order, forwards and backwards, including the two ends.
    for target in [3usize, 0, 6, 1, 5, 2, 6, 0, 4] {
        app.dispatch(Action::HistoryJump(target));
        assert_eq!(cursor(&app), target, "the cursor did not land on {target}");
        assert_eq!(
            print(&app),
            states[target],
            "jumping to state {target} produced a different document"
        );
    }
}

/// A new edit made after undoing throws the redo branch away. What it must not
/// do is leave the document in a state that belongs to the branch it dropped.
#[test]
fn editing_after_an_undo_drops_the_redo_branch_cleanly() {
    use cshop_core::filters::Filter;
    let Some(mut app) = app(80, 60) else { return };
    let start = print(&app);

    app.dispatch(Action::ApplyFilter(Box::new(Filter::GaussianBlur { radius: 6.0 })));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::FindEdges)));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Emboss { angle: 45.0, height: 3.0, amount: 1.0 })));
    assert_eq!(labels(&app).len(), 3);

    app.dispatch(Action::Undo);
    app.dispatch(Action::Undo);
    let after_two_undos = print(&app);

    // A different edit from here.
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));
    assert_eq!(labels(&app).len(), 2, "the redo branch should be gone: {:?}", labels(&app));
    assert!(!app.doc().unwrap().history.can_redo(), "and there should be nothing to redo");

    app.dispatch(Action::Undo);
    assert_eq!(print(&app), after_two_undos, "undoing the new edit went somewhere else");
    app.dispatch(Action::Undo);
    app.dispatch(Action::Undo);
    assert_eq!(print(&app), start, "and the whole way back is still the original");
}

/// Undo and redo past the ends must do nothing at all, rather than wrapping,
/// panicking, or quietly applying an entry twice.
#[test]
fn undo_and_redo_past_the_ends_are_no_ops() {
    use cshop_core::filters::Filter;
    let Some(mut app) = app(60, 50) else { return };
    let start = print(&app);

    for _ in 0..5 {
        app.dispatch(Action::Undo);
    }
    assert_eq!(print(&app), start, "undoing an empty history changed the document");
    assert_eq!(cursor(&app), 0);

    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));
    let inverted = print(&app);
    for _ in 0..5 {
        app.dispatch(Action::Redo);
    }
    assert_eq!(print(&app), inverted, "redoing past the end applied something again");
    assert_eq!(cursor(&app), 1);

    app.dispatch(Action::Undo);
    for _ in 0..5 {
        app.dispatch(Action::Undo);
    }
    assert_eq!(print(&app), start);
    app.dispatch(Action::Redo);
    assert_eq!(print(&app), inverted, "redo stopped working after undoing too far");
}

/// The stack drops its oldest entries to stay inside its memory budget. What
/// it must not do is let the cursor point at an entry that is no longer there.
#[test]
fn a_history_trimmed_by_its_budget_stays_consistent() {
    use cshop_core::filters::Filter;
    let Some(mut app) = app(300, 300) else { return };
    {
        // A budget small enough that a few full-layer filters overflow it.
        let view = app.doc_mut().unwrap();
        view.history = cshop_core::history::History::new("Open").with_budget(400_000);
    }

    for i in 0..12 {
        app.dispatch(Action::ApplyFilter(Box::new(Filter::AddNoise {
            amount: 0.3,
            monochromatic: false,
            gaussian: true,
            seed: i,
        })));
    }
    let (kept, forgotten) = {
        let h = &app.doc().unwrap().history;
        (h.labels().len(), h.forgotten())
    };
    assert!(forgotten > 0, "the budget should have dropped something, kept {kept}");
    assert_eq!(cursor(&app), kept, "the cursor must sit at the end of what is left");

    // Undoing all of what remains must not panic or run off the end.
    let deep = print(&app);
    for _ in 0..kept + 5 {
        app.dispatch(Action::Undo);
    }
    assert_eq!(cursor(&app), 0);
    for _ in 0..kept + 5 {
        app.dispatch(Action::Redo);
    }
    assert_eq!(cursor(&app), kept);
    assert_eq!(print(&app), deep, "redoing everything left did not come back to where it was");
}

// ---------------------------------------------------------------------------
// Smart objects and their shared sources
// ---------------------------------------------------------------------------

/// A placement is nine numbers and the source is a picture, so these undo
/// through two different mechanisms and both have to work.
#[test]
fn smart_object_edits_undo_exactly() {
    let Some(mut app) = app(120, 90) else { return };

    let steps: &[Step] = &[
        ("convert", |a| a.dispatch(Action::ConvertToSmartObject)),
        ("duplicate (shares the picture)", |a| a.dispatch(Action::DuplicateLayer)),
        ("scale the copy", |a| {
            let id = a.doc().unwrap().doc.active.unwrap();
            let view = a.doc_mut().unwrap();
            let dirty = view.history.apply(
                &mut view.doc,
                Box::new(cshop_core::history::PlaceSmart::new(
                    id,
                    cshop_core::transform::Transform::scale(0.5, 0.5),
                    (10, 10),
                    None,
                    cshop_core::resample::Resampling::Bilinear,
                    "Scale",
                )),
            );
            view.mark_dirty(dirty);
        }),
        ("make it unique", |a| a.dispatch(Action::MakeSmartUnique)),
        ("rasterise it", |a| a.dispatch(Action::RasterizeLayer)),
    ];
    let applied = round_trip(&mut app, steps);
    assert!(applied.len() >= 4, "these should all record history: {applied:?}");
}

// ---------------------------------------------------------------------------
// Selections, masks and channels
// ---------------------------------------------------------------------------

/// The selection is not pixels and is easy to get wrong: it is stored over its
/// own window, it prunes itself on undo, and several commands change it as a
/// side effect of doing something else.
#[test]
fn selection_and_mask_edits_undo_exactly() {
    use cshop_core::selection::{Rectf, Selection};
    let Some(mut app) = app(140, 100) else { return };

    let steps: &[Step] = &[
        ("select a rectangle", |a| {
            let sel = Selection::from_rect(
                140,
                100,
                Rectf::from_points(Vec2::new(25.0, 15.0), Vec2::new(100.0, 75.0)),
                true,
            );
            a.dispatch(Action::SetSelection(Box::new(sel), "Select"));
        }),
        ("feather it", |a| a.dispatch(Action::ModifySelection(cshop_ui::commands::ModifySelection::Feather(4.0)))),
        ("invert it", |a| a.dispatch(Action::InverseSelection)),
        ("mask from it", |a| a.dispatch(Action::AddLayerMaskFromSelection { invert: false })),
        ("paint on the mask", |a| {
            a.tool = cshop_ui::tools::Tool::Brush;
            a.brush.size = 20.0;
            a.begin_stroke(Vec2::new(60.0, 50.0), PaintMode::Paint);
            a.continue_stroke(Vec2::new(90.0, 60.0));
            a.end_stroke();
        }),
        ("apply the mask", |a| a.dispatch(Action::ApplyLayerMask)),
        ("deselect", |a| a.dispatch(Action::Deselect)),
    ];
    let applied = round_trip(&mut app, steps);
    assert!(applied.len() >= 5, "most of these should record history: {applied:?}");
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Resizing, cropping and transforming rewrite every layer at once, so their
/// undo has the most to put back.
#[test]
fn geometry_edits_undo_exactly() {
    let Some(mut app) = app(160, 120) else { return };

    let steps: &[Step] = &[
        ("second layer", |a| a.dispatch(Action::NewLayer)),
        ("paint on it", |a| {
            a.tool = cshop_ui::tools::Tool::Brush;
            a.brush.size = 24.0;
            a.foreground = Rgba8::opaque(240, 60, 20);
            a.begin_stroke(Vec2::new(40.0, 40.0), PaintMode::Paint);
            a.continue_stroke(Vec2::new(110.0, 80.0));
            a.end_stroke();
        }),
        ("image size", |a| a.dispatch(Action::ResizeImage {
            width: 96,
            height: 72,
            filter: cshop_core::resample::Resampling::Bicubic,
        })),
        ("canvas size", |a| a.dispatch(Action::ResizeCanvas {
            width: 130,
            height: 110,
            anchor: cshop_ui::commands::Anchor::TopLeft,
        })),
        ("rotate 90", |a| a.dispatch(Action::TransformPreset(cshop_ui::commands::TransformPreset::Rotate90Cw))),
        ("flip", |a| a.dispatch(Action::TransformPreset(cshop_ui::commands::TransformPreset::FlipHorizontal))),
        ("crop", |a| {
            let (w, h) = a.doc().map(|v| (v.doc.width, v.doc.height)).unwrap();
            let sel = cshop_core::selection::Selection::from_rect(
                w,
                h,
                cshop_core::selection::Rectf::from_points(Vec2::new(10.0, 8.0), Vec2::new(70.0, 60.0)),
                true,
            );
            a.dispatch(Action::SetSelection(Box::new(sel), "Select"));
            a.dispatch(Action::CropToSelection);
        }),
    ];
    let applied = round_trip(&mut app, steps);
    assert!(applied.len() >= 6, "these all change geometry: {applied:?}");
}

// ---------------------------------------------------------------------------
// The length the stack is actually asked for
// ---------------------------------------------------------------------------

/// Three hundred edits of every kind, undone and redone in full.
///
/// States are keyed by the cursor rather than by the loop index, because a
/// dispatch can record nothing, can merge into the entry before it, and can
/// record more than one entry. The cursor is the only thing that says where in
/// the history we are.
#[test]
fn three_hundred_edits_undo_and_redo_in_full() {
    use cshop_core::filters::Filter;
    use cshop_ui::tools::Tool;
    use std::collections::HashMap;

    let Some(mut app) = app(180, 140) else { return };
    {
        // Room for the whole chain: once the stack starts dropping its oldest
        // entries the cursor is pulled back with every drop and stops being a
        // unique name for a state. Trimming has its own test.
        let view = app.doc_mut().unwrap();
        view.history = cshop_core::history::History::new("Open").with_limit(1000);
    }

    let tools = [Tool::Brush, Tool::Eraser, Tool::Pencil, Tool::Dodge, Tool::Burn, Tool::Blur];
    let filters = [
        Filter::Solarize,
        Filter::FindEdges,
        Filter::GaussianBlur { radius: 3.0 },
        Filter::Emboss { angle: 45.0, height: 2.0, amount: 1.0 },
    ];

    let mut by_cursor: HashMap<usize, u64> = HashMap::new();
    by_cursor.insert(0, print(&app));

    for i in 0..300 {
        match i % 10 {
            0 => app.dispatch(Action::NewLayer),
            3 => app.dispatch(Action::ApplyFilter(Box::new(filters[i / 10 % 4].clone()))),
            6 => {
                let id = app.doc().unwrap().doc.active.unwrap();
                app.dispatch(Action::SetLayerProperty(
                    id,
                    cshop_core::history::LayerProperty::Opacity(0.3 + (i % 7) as f32 * 0.1),
                ));
            }
            8 => app.dispatch(Action::MergeDown),
            _ => {
                app.tool = tools[i % tools.len()];
                app.brush.size = 5.0 + (i % 6) as f32 * 4.0;
                app.foreground = Rgba8::opaque((i * 31 % 255) as u8, 120, (i * 17 % 255) as u8);
                let (x, y) = (8.0 + (i % 9) as f32 * 18.0, 8.0 + (i % 11) as f32 * 11.0);
                app.begin_stroke(Vec2::new(x, y), PaintMode::Paint);
                app.continue_stroke(Vec2::new(x + 22.0, y + 9.0));
                app.end_stroke();
            }
        }
        by_cursor.insert(cursor(&app), print(&app));
    }

    let deep = print(&app);
    let entries = cursor(&app);
    assert!(entries > 250, "the chain should have filled the stack, got {entries}");
    assert_eq!(app.doc().unwrap().history.forgotten(), 0, "nothing should have been dropped");

    let mut compared = 0;
    while app.doc().unwrap().history.can_undo() {
        app.dispatch(Action::Undo);
        // A cursor whose state was overwritten by a later edit at the same
        // depth is not comparable; that happens wherever an entry merged.
        if let Some(want) = by_cursor.get(&cursor(&app)) {
            assert_eq!(print(&app), *want, "the document drifted undoing to cursor {}", cursor(&app));
            compared += 1;
        }
    }
    assert!(compared > 250, "most states should have been comparable, got {compared}");

    while app.doc().unwrap().history.can_redo() {
        app.dispatch(Action::Redo);
    }
    assert_eq!(print(&app), deep, "redoing everything did not come back to where it was");
}

// ---------------------------------------------------------------------------
// Undo against work still running on a thread
// ---------------------------------------------------------------------------

/// The window puts long filters on a worker, so a user can press Ctrl+Z while
/// one is still out. Whatever that filter computed is now against pixels that
/// no longer exist, and writing it back would undo the undo.
#[test]
fn undo_while_a_filter_is_still_running() {
    use cshop_core::filters::Filter;
    let Some(mut app) = worker_app(900) else { return };
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));
    settle(&mut app);
    let solarized = print(&app);
    let start = {
        app.dispatch(Action::Undo);
        let s = print(&app);
        app.dispatch(Action::Redo);
        s
    };

    app.dispatch(Action::ApplyFilter(Box::new(Filter::SurfaceBlur {
        radius: 12.0,
        threshold: 0.25,
    })));
    assert!(app.jobs.any(), "the blur should still be out");
    app.dispatch(Action::Undo);
    assert_eq!(print(&app), start, "the undo should have landed straight away");

    settle(&mut app);
    assert_eq!(
        print(&app),
        start,
        "the filter came back and wrote itself over an undo"
    );
    assert_eq!(
        app.doc().unwrap().history.labels(),
        vec!["Solarize"],
        "and it should not have recorded itself either"
    );
    let _ = solarized;
}

/// Undo and redo back to where the filter started from, and it *should* land:
/// the pixels it was computed against are there again.
#[test]
fn undo_then_redo_lets_a_running_filter_land() {
    use cshop_core::filters::Filter;
    let Some(mut app) = worker_app(900) else { return };
    app.dispatch(Action::ApplyFilter(Box::new(Filter::Solarize)));
    settle(&mut app);

    app.dispatch(Action::ApplyFilter(Box::new(Filter::SurfaceBlur {
        radius: 12.0,
        threshold: 0.25,
    })));
    app.dispatch(Action::Undo);
    app.dispatch(Action::Redo);
    settle(&mut app);
    assert_eq!(
        app.doc().unwrap().history.labels(),
        vec!["Solarize", "Surface Blur"],
        "the filter should have landed once its pixels were back"
    );
}

/// A second filter asked for while one is running is refused rather than
/// replacing it, so the history cannot end up with the wrong one.
#[test]
fn a_second_filter_while_one_runs_is_refused() {
    use cshop_core::filters::Filter;
    let Some(mut app) = worker_app(900) else { return };
    app.dispatch(Action::ApplyFilter(Box::new(Filter::SurfaceBlur {
        radius: 12.0,
        threshold: 0.25,
    })));
    app.dispatch(Action::ApplyFilter(Box::new(Filter::FindEdges)));
    settle(&mut app);
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Surface Blur"]);
}

// ---------------------------------------------------------------------------
// The optional models
// ---------------------------------------------------------------------------

/// Every model-driven edit, applied and undone.
///
/// Skipped without the vision pack, which is optional and is not installed in
/// most places this will run.
#[test]
fn model_driven_edits_undo_exactly() {
    if !cshop_ui::vision::is_available() {
        eprintln!("no vision pack; skipping");
        return;
    }
    // Each of these is its own document, because a model's answer depends on
    // what it is shown and chaining them would test the models rather than
    // the undo stack.
    // Setting up (a selection, say) is not part of the gesture, so it happens
    // before the mark is taken and does not count against it.
    type Setup = fn(&mut CShopApp);
    type Step = (&'static str, Setup, Setup);
    let nothing: Setup = |_| {};
    let each: &[Step] = &[
        (
            "fill in",
            |a| {
                let (w, h) = a.doc().map(|v| (v.doc.width, v.doc.height)).unwrap();
                let sel = cshop_core::selection::Selection::from_rect(
                    w,
                    h,
                    cshop_core::selection::Rectf::from_points(
                        Vec2::new(w as f32 / 2.0 - 20.0, h as f32 / 2.0 - 20.0),
                        Vec2::new(w as f32 / 2.0 + 20.0, h as f32 / 2.0 + 20.0),
                    ),
                    true,
                );
                a.dispatch(Action::SetSelection(Box::new(sel), "Select"));
            },
            |a| a.dispatch(Action::FillInSelection),
        ),
        ("mask from depth", nothing, |a| {
            a.dispatch(Action::AddLayerMaskFromDepth { invert: false })
        }),
        ("remove noise", |a| a.dispatch(Action::ShowDenoise), |a| {
            a.dispatch(Action::RunDenoise);
            a.dispatch(Action::DenoiseKeep);
        }),
        ("relight", |a| a.dispatch(Action::ShowRelight), |a| {
            a.dispatch(Action::RelightPreview);
            a.dispatch(Action::RelightKeep);
        }),
        ("replace sky", nothing, |a| a.dispatch(Action::ReplaceSky)),
    ];

    for (name, setup, step) in each {
        let Some(mut fresh) = app(180, 140) else { return };
        // Before the setup, because opening one of these windows puts a
        // preview straight onto the layer without recording it — so the
        // document with the window open is not a state undo will ever return
        // to. The pristine document is.
        let pristine = print(&fresh);
        setup(&mut fresh);
        let at = cursor(&fresh);
        step(&mut fresh);
        if cursor(&fresh) == at {
            continue; // the model found nothing to do here
        }
        assert_eq!(
            cursor(&fresh) - at,
            1,
            "{name} recorded {} entries; one gesture should be one undo",
            cursor(&fresh) - at
        );
        while fresh.doc().unwrap().history.can_undo() {
            fresh.dispatch(Action::Undo);
        }
        assert_eq!(print(&fresh), pristine, "undoing {name} left the document different");
    }
}
