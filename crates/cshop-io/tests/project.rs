//! The native project format must give back exactly what it was given.

use cshop_core::adjust::Adjustment;
use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::effects::*;
use cshop_core::geom::IRect;
use cshop_core::layer::{Layer, LayerKind, LayerMask};
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;
use cshop_core::shape::{ShapeContent, ShapeKind, ShapeStyle, StrokeAlign};
use cshop_core::text::{TextContent, TextStyle};

/// A document exercising every layer kind, a group, a mask, all ten effects
/// and a saved channel — so a round trip has something to lose.
fn rich_document() -> Document {
    let mut doc = Document::new("Project", 120, 90, Background::White);
    doc.dpi = 144.0;

    let id = doc.tree.alloc_id();
    let mut px = PixelBuffer::filled(60, 40, Rgba8::opaque(10, 120, 200));
    px.fill_rect(IRect::at(5, 5, 10, 10), Rgba8::opaque(250, 30, 40));
    let mut layer = Layer::raster(id, "Painted", px);
    layer.offset = (7, 11);
    layer.opacity = 0.62;
    layer.fill_opacity = 0.41;
    layer.blend_mode = BlendMode::Multiply;
    layer.clipping = true;
    layer.locks.transparency = true;
    layer.locks.position = true;
    let mut mask = MaskBuffer::new(20, 20, 128);
    mask.set(3, 3, 255);
    layer.mask = Some(LayerMask { data: mask, offset: (2, 3), enabled: false, linked: false });
    layer.effects = LayerEffects {
        enabled: true,
        global_light_angle: 47.0,
        global_light_altitude: 22.0,
        drop_shadow: Some(Shadow { distance: 9.0, spread: 0.25, ..Default::default() }),
        outer_glow: Some(Glow { size: 13.0, ..Default::default() }),
        bevel: Some(Bevel { style: BevelStyle::Pillow, depth: 2.5, ..Default::default() }),
        inner_shadow: Some(Shadow::default()),
        inner_glow: Some(Glow { source: GlowSource::Center, ..Default::default() }),
        satin: Some(Satin::default()),
        color_overlay: Some(ColorOverlay::default()),
        gradient_overlay: Some(GradientOverlay {
            kind: cshop_core::fill::GradientKind::Diamond,
            reverse: true,
            ..Default::default()
        }),
        pattern_overlay: Some(PatternOverlay {
            kind: PatternKind::CrossHatch,
            seed: 4321,
            ..Default::default()
        }),
        stroke: Some(Stroke { position: StrokePosition::Inside, ..Default::default() }),
    };
    doc.tree.push(layer, None);

    let gid = doc.tree.alloc_id();
    let mut group = Layer::group(gid, "Group");
    group.expanded = false;
    doc.tree.push(group, None);
    let cid = doc.tree.alloc_id();
    doc.tree.push(Layer::raster(cid, "Inside", PixelBuffer::filled(8, 8, Rgba8::BLACK)), Some(gid));

    let aid = doc.tree.alloc_id();
    let mut curves: [cshop_core::curve::Curve; 4] = Default::default();
    curves[0] = cshop_core::curve::Curve::new(vec![(0.0, 0.1), (0.5, 0.7), (1.0, 0.9)]);
    doc.tree.push(Layer::adjustment(aid, Adjustment::Curves { curves }), None);

    let fid = doc.tree.alloc_id();
    doc.tree.push(
        Layer::new(
            fid,
            "Fill",
            LayerKind::Fill(cshop_core::layer::FillStyle::Solid(Rgba8::opaque(9, 9, 9))),
        ),
        None,
    );

    if !cshop_core::font::FontDb::global().families().is_empty() {
        let tid = doc.tree.alloc_id();
        let content = TextContent {
            text: "Round trip".into(),
            style: TextStyle {
                family: cshop_core::font::FontDb::global().default_family(),
                size: 31.0,
                tracking: 40.0,
                leading: Some(44.0),
                bold: true,
                ..Default::default()
            },
            wrap_width: Some(200.0),
        };
        if let Some(l) = Layer::text_layer(tid, content) {
            doc.tree.push(l, None);
        }
    }

    let sid = doc.tree.alloc_id();
    let shape = ShapeContent::new(
        ShapeKind::Polygon { sides: 7, star: true, inner: 0.37 },
        (44.0, 33.0),
        ShapeStyle {
            fill: Some(Rgba8::opaque(200, 200, 30)),
            stroke: Some(Rgba8::BLACK),
            stroke_width: 3.5,
            stroke_align: StrokeAlign::Outside,
            antialias: false,
        },
    );
    if let Some(l) = Layer::shape_layer(sid, shape) {
        doc.tree.push(l, None);
    }

    doc.channels.push(cshop_core::document::AlphaChannel {
        name: "Alpha 1".into(),
        data: MaskBuffer::new(120, 90, 77),
        visible: true,
    });
    doc.active = Some(aid);
    doc
}

fn round_trip(doc: &Document) -> Document {
    cshop_io::project::read(&cshop_io::project::write(doc)).expect("the project should read back")
}

#[test]
fn every_layer_property_survives_a_round_trip() {
    let before = rich_document();
    let after = round_trip(&before);

    assert_eq!((after.width, after.height), (before.width, before.height));
    assert_eq!(after.name, before.name);
    assert_eq!(after.dpi, before.dpi);
    assert_eq!(after.active, before.active);
    assert_eq!(after.tree.len(), before.tree.len(), "every layer should come back");
    assert!(!after.modified, "a freshly opened project is not modified");

    for id in before.tree.iter_all() {
        let a = before.tree.get(id).expect("before");
        let b = after.tree.get(id).unwrap_or_else(|| panic!("layer {id:?} was lost"));
        assert_eq!(a.name, b.name);
        assert_eq!(a.visible, b.visible, "{}", a.name);
        assert_eq!(a.opacity, b.opacity, "{}", a.name);
        assert_eq!(a.fill_opacity, b.fill_opacity, "{}", a.name);
        assert_eq!(a.blend_mode, b.blend_mode, "{}", a.name);
        assert_eq!(a.offset, b.offset, "{}", a.name);
        assert_eq!(a.clipping, b.clipping, "{}", a.name);
        assert_eq!(a.expanded, b.expanded, "{}", a.name);
        assert_eq!(a.locks, b.locks, "{}", a.name);
        assert_eq!(a.effects, b.effects, "{} lost its effects", a.name);
        assert_eq!(a.parent, b.parent, "{} changed parent", a.name);
        assert_eq!(a.kind.type_name(), b.kind.type_name(), "{} changed kind", a.name);
        match (&a.mask, &b.mask) {
            (None, None) => {}
            (Some(x), Some(y)) => {
                assert_eq!((x.offset, x.enabled, x.linked), (y.offset, y.enabled, y.linked));
                assert_eq!(x.data.as_bytes(), y.data.as_bytes(), "{} lost mask pixels", a.name);
            }
            _ => panic!("{} gained or lost its mask", a.name),
        }
        if let (Some(x), Some(y)) = (a.pixels(), b.pixels()) {
            assert_eq!(x.as_bytes(), y.as_bytes(), "{} lost pixels", a.name);
        }
    }
}

#[test]
fn the_tree_keeps_its_shape_and_order() {
    let before = rich_document();
    let after = round_trip(&before);
    assert_eq!(after.tree.root().len(), before.tree.root().len());
    // Order matters: layer order is stacking order.
    let names = |d: &Document| -> Vec<String> {
        d.tree.iter_all().iter().map(|id| d.tree.get(*id).unwrap().name.clone()).collect()
    };
    assert_eq!(names(&after), names(&before));
}

#[test]
fn live_type_and_shapes_stay_editable() {
    let before = rich_document();
    let after = round_trip(&before);
    let mut checked = 0;
    for id in before.tree.iter_all() {
        let (a, b) = (before.tree.get(id).unwrap(), after.tree.get(id).unwrap());
        if let (Some(x), Some(y)) = (a.text(), b.text()) {
            assert_eq!(x.content(), y.content(), "the type layer lost its content");
            checked += 1;
        }
        if let (Some(x), Some(y)) = (a.shape(), b.shape()) {
            assert_eq!(x.content(), y.content(), "the shape layer lost its geometry");
            checked += 1;
        }
    }
    assert!(checked > 0, "the fixture should have had a vector layer to check");
}

#[test]
fn adjustments_and_channels_survive() {
    let after = round_trip(&rich_document());
    let found = after
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| after.tree.get(id))
        .find_map(|l| l.adjustment_settings().cloned())
        .expect("the adjustment layer should still be one");
    match found {
        Adjustment::Curves { curves } => {
            assert_eq!(curves[0].points().len(), 3, "the curve kept its points");
            assert!((curves[0].points()[1].1 - 0.7).abs() < 1e-6);
        }
        other => panic!("the adjustment changed into {other:?}"),
    }

    assert_eq!(after.channels.len(), 1);
    assert_eq!(after.channels[0].name, "Alpha 1");
    assert_eq!(after.channels[0].data.get(4, 4), 77);
}

/// A project file is an attack surface, so a damaged one must be refused
/// rather than crashing or allocating wildly.
#[test]
fn damaged_files_are_refused_not_fatal() {
    assert!(cshop_io::project::read(b"").is_err(), "empty");
    assert!(cshop_io::project::read(b"not a project at all").is_err(), "wrong magic");

    let good = cshop_io::project::write(&rich_document());

    // Truncated at every length: none may panic.
    for cut in (0..good.len()).step_by(97) {
        let _ = cshop_io::project::read(&good[..cut]);
    }

    // A future version is refused with an explanation rather than misread.
    let mut newer = good.clone();
    newer[6] = 0xff;
    newer[7] = 0xff;
    let err = cshop_io::project::read(&newer).unwrap_err();
    assert!(format!("{err}").contains("newer version"), "got {err}");

    // Flipped bytes in the body: must error, never panic.
    for at in (12..good.len()).step_by(211) {
        let mut bad = good.clone();
        bad[at] ^= 0xa5;
        let _ = cshop_io::project::read(&bad);
    }
}

/// Unknown chunks are for forward compatibility: an older build should keep
/// what it understands rather than refusing the file.
#[test]
fn an_unknown_chunk_is_skipped() {
    let doc = rich_document();
    let mut bytes = cshop_io::project::write(&doc);
    // Splice a chunk this version has never heard of in just after the header.
    let mut spliced = bytes[..8].to_vec();
    spliced.extend_from_slice(b"XZZY");
    spliced.extend_from_slice(&12u32.to_le_bytes());
    spliced.extend_from_slice(&[0xab; 12]);
    spliced.extend_from_slice(&bytes[8..]);
    bytes = spliced;

    let after = cshop_io::project::read(&bytes).expect("the unknown chunk should be skipped");
    assert_eq!(after.tree.len(), doc.tree.len());
}

/// Through the file-level entry points, which is how the application reaches
/// them: the extension alone decides which format is written and read.
#[test]
fn documents_round_trip_through_files() {
    let dir = std::env::temp_dir().join("cshop-format-test");
    let _ = std::fs::create_dir_all(&dir);
    let doc = rich_document();
    let composite = PixelBuffer::filled(doc.width, doc.height, Rgba8::opaque(3, 4, 5));

    // The native format keeps everything.
    let path = dir.join("round.cshop");
    cshop_io::save_document(&path, &doc, &composite).expect("save");
    let back = cshop_io::load_document(&path).expect("load");
    assert_eq!(back.tree.len(), doc.tree.len());
    assert_eq!(back.path.as_deref(), Some(path.as_path()));
    assert!(back
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| back.tree.get(id))
        .any(|l| l.effects.any()), "the styles should have come back");

    // PSD keeps the stack, flattened to raster.
    let path = dir.join("round.psd");
    cshop_io::save_document(&path, &doc, &composite).expect("save psd");
    let back = cshop_io::load_document(&path).expect("load psd");
    assert!(back.tree.len() > 1, "PSD should carry more than one layer");

    // A flat format writes the composite and reads back as one layer.
    let path = dir.join("round.png");
    cshop_io::save_document(&path, &doc, &composite).expect("save png");
    let back = cshop_io::load_document(&path).expect("load png");
    assert_eq!(back.tree.len(), 1, "a PNG comes back as a single layer");
    assert_eq!(
        back.tree.get(back.tree.root()[0]).unwrap().pixels().unwrap().get(1, 1),
        Rgba8::opaque(3, 4, 5),
        "and holds the composite it was given"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Whatever the extension says, the bytes decide: a project renamed to `.png`
/// still opens as a project.
#[test]
fn the_format_is_recognised_from_the_bytes() {
    let doc = rich_document();
    let bytes = cshop_io::project::write(&doc);
    let back = cshop_io::decode_document(&bytes, Some(std::path::Path::new("lying.png")))
        .expect("the magic should win over the extension");
    assert_eq!(back.tree.len(), doc.tree.len());
}

/// A path shape, with a boolean operand, has to survive the round trip.
#[test]
fn a_compound_path_survives_the_round_trip() {
    use cshop_core::path::{BoolOp, PathPart, PathShape};
    use cshop_core::shape::{outline, ShapeContent, ShapeKind, ShapeStyle};

    let mut doc = Document::new("paths", 200, 200, Background::White);
    let mut shape = PathShape::new(outline(&ShapeKind::Ellipse, (80.0, 60.0)));
    shape.parts.push(PathPart {
        subpaths: outline(&ShapeKind::Rectangle { radius: 6.0 }, (40.0, 40.0)),
        op: BoolOp::Subtract,
    });
    let content = ShapeContent::new(ShapeKind::Path(shape.clone()), (80.0, 60.0), ShapeStyle::default());
    let id = doc.tree.alloc_id();
    let layer = cshop_core::layer::Layer::shape_layer(id, content).expect("a shape layer");
    doc.tree.push(layer, None);

    let bytes = cshop_io::project::write(&doc);
    let back = cshop_io::project::read(&bytes).expect("should read back");

    let restored = back
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| back.tree.get(id))
        .find_map(|l| l.shape().map(|s| s.content().clone()))
        .expect("the shape layer should have come back");
    match restored.kind {
        ShapeKind::Path(p) => {
            assert_eq!(p, shape, "every anchor and operation should be identical");
            assert_eq!(p.parts[1].op, BoolOp::Subtract);
        }
        other => panic!("came back as {}", other.name()),
    }
}

/// Guides belong to the document, so they have to survive a save.
#[test]
fn guides_come_back_where_they_were_put() {
    use cshop_core::guides::Guide;
    let mut doc = rich_document();
    doc.guides = vec![Guide::vertical(120.5), Guide::horizontal(40.0), Guide::vertical(0.0)];

    let back = cshop_io::project::read(&cshop_io::project::write(&doc)).expect("read");
    assert_eq!(back.guides, doc.guides);
}

/// A document with none writes no chunk, and one written before guides
/// existed still opens.
#[test]
fn a_document_without_guides_carries_none() {
    let doc = rich_document();
    assert!(doc.guides.is_empty());
    let bytes = cshop_io::project::write(&doc);
    assert!(
        !bytes.windows(4).any(|w| w == b"GIDE"),
        "nothing to say, so nothing should be said"
    );
    assert!(cshop_io::project::read(&bytes).expect("read").guides.is_empty());
}

/// A count is a claim the file makes about itself, so it is bounded by what
/// the chunk could hold rather than believed.
#[test]
fn a_guide_count_that_lies_is_not_believed() {
    use cshop_core::guides::Guide;
    let mut doc = rich_document();
    doc.guides = vec![Guide::vertical(10.0)];
    let mut bytes = cshop_io::project::write(&doc);

    let at = bytes.windows(4).position(|w| w == b"GIDE").expect("the chunk");
    // Claim a million guides in a chunk that holds one.
    bytes[at + 8..at + 12].copy_from_slice(&1_000_000u32.to_le_bytes());
    let back = cshop_io::project::read(&bytes).expect("it should still open");
    assert!(back.guides.len() <= 1, "it read {} of them", back.guides.len());
}

/// A smart object saves its source and its placement, not its rendering: the
/// rendering can be worked out exactly and would double the file.
#[test]
fn a_smart_object_survives_a_save_and_reopen() {
    use cshop_core::layer::{Layer, LayerKind};
    use cshop_core::resample::Resampling;
    use cshop_core::smart::SmartObject;
    use cshop_core::transform::Transform;

    let mut source = cshop_core::pixels::PixelBuffer::new(48, 32);
    for y in 0..32 {
        for x in 0..48 {
            let on = (x / 2 + y / 2) % 2 == 0;
            source.set(x, y, if on { Rgba8::WHITE } else { Rgba8::BLACK });
        }
    }
    let mut smart = SmartObject::new(source.clone());
    smart.place(Transform::scale(0.5, 0.5), Resampling::Bilinear);

    let mut doc = Document::new("smart", 100, 100, Background::Transparent);
    let id = doc.tree.alloc_id();
    doc.tree.push(Layer::new(id, "Placed", LayerKind::Smart(Box::new(smart))), None);

    let back = round_trip(&doc);
    let layer = back
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|i| back.tree.get(i))
        .find(|l| l.name == "Placed")
        .expect("the layer should have come back");
    let reopened = layer.smart().expect("it should still be a smart object");

    assert_eq!(reopened.source().pixels(), source.pixels(), "the source, sample for sample");
    assert_eq!(reopened.raster().width(), 24, "re-rendered at the placement it had");
    // And still reversible, which is the whole point of keeping the source.
    let mut again = reopened.clone();
    again.place(Transform::IDENTITY, Resampling::Bilinear);
    assert_eq!(again.raster().pixels(), source.pixels());
}
