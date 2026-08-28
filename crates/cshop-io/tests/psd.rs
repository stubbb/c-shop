//! PSD reading and writing.

use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_core::layer::{Layer, LayerMask};
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;

fn doc_with_layers() -> (Document, PixelBuffer) {
    let mut doc = Document::new("Psd", 64, 48, Background::Transparent);
    // `Document::new` seeds a layer of its own; these tests want only theirs.
    doc.tree = Default::default();

    let id = doc.tree.alloc_id();
    let mut base = Layer::raster(id, "Base", PixelBuffer::filled(64, 48, Rgba8::opaque(30, 60, 90)));
    base.is_background = true;
    doc.tree.push(base, None);

    let id = doc.tree.alloc_id();
    let mut px = PixelBuffer::filled(20, 16, Rgba8::opaque(220, 40, 40));
    px.fill_rect(IRect::at(0, 0, 4, 4), Rgba8::new(0, 0, 0, 0));
    let mut top = Layer::raster(id, "Patch", px);
    top.offset = (10, 8);
    top.opacity = 0.5;
    top.blend_mode = BlendMode::Multiply;
    top.visible = false;
    top.clipping = true;
    let mut mask = MaskBuffer::new(20, 16, 200);
    mask.set(1, 1, 0);
    top.mask = Some(LayerMask { data: mask, offset: (10, 8), enabled: false, linked: true });
    doc.tree.push(top, None);

    let composite = PixelBuffer::filled(64, 48, Rgba8::opaque(30, 60, 90));
    (doc, composite)
}

#[test]
fn a_psd_round_trips_its_layers() {
    let (doc, composite) = doc_with_layers();
    let bytes = cshop_io::psd::write(&doc, &composite).expect("write");
    assert_eq!(&bytes[..4], b"8BPS", "the file should be signed");

    let back = cshop_io::psd::read(&bytes).expect("read");
    assert_eq!((back.width, back.height), (64, 48));
    assert_eq!(back.tree.root().len(), 2, "both layers should come back");

    let names: Vec<String> =
        back.tree.iter_all().iter().map(|id| back.tree.get(*id).unwrap().name.clone()).collect();
    assert_eq!(names, vec!["Base", "Patch"], "in the order they were stacked");

    let patch = back.tree.get(back.tree.root()[1]).unwrap();
    assert_eq!(patch.offset, (10, 8), "the layer keeps its position");
    assert_eq!(patch.blend_mode, BlendMode::Multiply);
    assert!(!patch.visible, "hidden layers stay hidden");
    assert!(patch.clipping);
    assert!((patch.opacity - 0.5).abs() < 0.01, "opacity was {}", patch.opacity);
    assert!(patch.mask.is_some(), "the layer mask should survive");
    let mask = patch.mask.as_ref().unwrap();
    assert!(!mask.enabled, "a disabled mask stays disabled");
    assert_eq!(mask.data.get(1, 1), 0);
}

#[test]
fn layer_pixels_survive_the_round_trip() {
    let (doc, composite) = doc_with_layers();
    let back = cshop_io::psd::read(&cshop_io::psd::write(&doc, &composite).unwrap()).unwrap();
    let before = doc.tree.get(doc.tree.root()[1]).unwrap().pixels().unwrap();
    let after = back.tree.get(back.tree.root()[1]).unwrap().pixels().unwrap();
    assert_eq!(after.as_bytes(), before.as_bytes(), "RLE must be lossless");
}

/// PSD has no nesting: a group is a pair of markers around its children, so
/// this is the part most likely to come back inside out.
#[test]
fn groups_survive_as_groups() {
    let mut doc = Document::new("Groups", 32, 32, Background::Transparent);
    doc.tree = Default::default();
    let gid = doc.tree.alloc_id();
    let mut group = Layer::group(gid, "Folder");
    group.expanded = false;
    doc.tree.push(group, None);
    for name in ["Inner A", "Inner B"] {
        let id = doc.tree.alloc_id();
        doc.tree.push(
            Layer::raster(id, name, PixelBuffer::filled(8, 8, Rgba8::opaque(1, 2, 3))),
            Some(gid),
        );
    }
    let id = doc.tree.alloc_id();
    doc.tree.push(
        Layer::raster(id, "Outside", PixelBuffer::filled(8, 8, Rgba8::WHITE)),
        None,
    );

    let composite = PixelBuffer::filled(32, 32, Rgba8::WHITE);
    let back = cshop_io::psd::read(&cshop_io::psd::write(&doc, &composite).unwrap()).unwrap();

    assert_eq!(back.tree.root().len(), 2, "a group and a loose layer at the root");
    let folder = back
        .tree
        .root()
        .iter()
        .find(|id| back.tree.get(**id).unwrap().name == "Folder")
        .copied()
        .expect("the group should come back named");
    assert!(back.tree.get(folder).unwrap().kind.is_group());
    assert!(!back.tree.get(folder).unwrap().expanded, "a closed group stays closed");

    let children: Vec<String> = back
        .tree
        .children(Some(folder))
        .iter()
        .map(|id| back.tree.get(*id).unwrap().name.clone())
        .collect();
    assert_eq!(children, vec!["Inner A", "Inner B"], "the children stay inside, in order");
}

#[test]
fn every_blend_mode_maps_both_ways() {
    for mode in BlendMode::all() {
        let mut doc = Document::new("B", 4, 4, Background::Transparent);
        doc.tree = Default::default();
        let id = doc.tree.alloc_id();
        let mut l = Layer::raster(id, "L", PixelBuffer::filled(4, 4, Rgba8::WHITE));
        l.blend_mode = mode;
        doc.tree.push(l, None);
        let bytes =
            cshop_io::psd::write(&doc, &PixelBuffer::filled(4, 4, Rgba8::WHITE)).unwrap();
        let back = cshop_io::psd::read(&bytes).unwrap();
        let got = back.tree.get(back.tree.root()[0]).unwrap().blend_mode;
        assert_eq!(got, mode, "{mode:?} did not survive the trip through PSD");
    }
}

/// Readers that ignore layers show the composite, so it has to be right.
#[test]
fn the_flattened_composite_is_written_and_read_back() {
    let mut doc = Document::new("C", 16, 12, Background::Transparent);
    let mut composite = PixelBuffer::filled(16, 12, Rgba8::opaque(12, 34, 56));
    composite.fill_rect(IRect::at(2, 2, 5, 5), Rgba8::opaque(200, 100, 50));
    // No layers at all, so reading falls back to the composite.
    doc.tree = Default::default();

    let bytes = cshop_io::psd::write(&doc, &composite).unwrap();
    let back = cshop_io::psd::read(&bytes).unwrap();
    assert_eq!(back.tree.len(), 1, "the composite becomes one layer");
    let px = back.tree.get(back.tree.root()[0]).unwrap().pixels().unwrap();
    assert_eq!(px.get(0, 0), Rgba8::opaque(12, 34, 56));
    assert_eq!(px.get(4, 4), Rgba8::opaque(200, 100, 50));
}

/// PackBits is the format's compression; a bad implementation shows up as
/// stripes or shifted rows rather than an error.
#[test]
fn packbits_is_lossless_on_awkward_data() {
    let mut doc = Document::new("R", 130, 3, Background::Transparent);
    doc.tree = Default::default();
    let mut px = PixelBuffer::new(130, 3);
    for y in 0..3i32 {
        for x in 0..130i32 {
            // Long runs, single outliers and alternating pairs: the three
            // shapes a run-length coder gets wrong.
            let v = match x % 40 {
                0..=30 => 7u8,
                31 => 250,
                _ => (x % 2 * 255) as u8,
            };
            px.set(x, y, Rgba8::new(v, v.wrapping_add(3), v.wrapping_mul(2), 255));
        }
    }
    let id = doc.tree.alloc_id();
    doc.tree.push(Layer::raster(id, "Runs", px.clone()), None);

    let back = cshop_io::psd::read(&cshop_io::psd::write(&doc, &px).unwrap()).unwrap();
    let after = back.tree.get(back.tree.root()[0]).unwrap().pixels().unwrap();
    assert_eq!(after.as_bytes(), px.as_bytes());
}

#[test]
fn damaged_psd_files_are_refused_not_fatal() {
    assert!(cshop_io::psd::read(b"").is_err());
    assert!(cshop_io::psd::read(b"8BPSnonsense").is_err());

    let (doc, composite) = doc_with_layers();
    let good = cshop_io::psd::write(&doc, &composite).unwrap();
    for cut in (0..good.len()).step_by(53) {
        let _ = cshop_io::psd::read(&good[..cut]);
    }
    for at in (0..good.len()).step_by(131) {
        let mut bad = good.clone();
        bad[at] ^= 0x5a;
        let _ = cshop_io::psd::read(&bad);
    }
}

#[test]
fn an_oversized_canvas_is_refused_with_an_explanation() {
    let doc = Document::new("Big", 40_000, 10, Background::Transparent);
    let err = cshop_io::psd::write(&doc, &PixelBuffer::new(40_000, 10)).unwrap_err();
    assert!(format!("{err}").contains("30000"), "got {err}");
}
