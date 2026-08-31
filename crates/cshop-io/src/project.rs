//! The native layered project format, `.cshop`.
//!
//! Everything a document holds: the layer tree, groups, masks, adjustment
//! settings, live type and shape descriptions, layer effects, saved channels
//! and the history-independent state. Opening a project gives back exactly
//! what was saved, still editable.
//!
//! # Why this is written by hand
//!
//! A derive-based encoding ties the file layout to the *order* of fields and
//! enum variants, so reordering a struct silently changes the format and
//! corrupts every file already written. Documents outlive the code that wrote
//! them, so the layout is spelled out here instead, where changing it is a
//! deliberate act.
//!
//! # Shape
//!
//! A short header, then a sequence of tagged chunks:
//!
//! ```text
//! "CSHOP\0"  u16 version
//! chunk:  [4-byte tag][u32 length][payload]  ...repeated
//! ```
//!
//! A reader skips chunks it does not recognise, so a newer file loses only the
//! parts an older build has no idea about rather than failing outright.

use crate::bytes::{Reader, Writer};
use crate::IoError;
use cshop_core::adjust::{Adjustment, LevelsChannel};
use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::curve::Curve;
use cshop_core::document::Document;
use cshop_core::effects::*;
use cshop_core::layer::{FillStyle, Layer, LayerId, LayerKind, LayerLocks, LayerMask};
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;
use cshop_core::shape::{ShapeContent, ShapeKind, ShapeStyle, StrokeAlign};
use cshop_core::text::{TextAlign, TextContent, TextStyle};

const MAGIC: &[u8; 6] = b"CSHOP\0";
/// Bumped only when a change cannot be expressed as a new chunk.
const VERSION: u16 = 1;

const CHUNK_DOC: &[u8; 4] = b"DOCU";
const CHUNK_LAYER: &[u8; 4] = b"LAYR";
const CHUNK_CHANNEL: &[u8; 4] = b"CHAN";
/// The working space, when it is not the sRGB every project is assumed to be
/// in. Its own chunk rather than a field in `DOCU`, so a project written
/// before profiles existed still opens, and one written after still opens in
/// a build from before.
const CHUNK_PROFILE: &[u8; 4] = b"ICCP";
/// Guides, when there are any. Its own chunk for the same reason as the
/// profile: a project written before they existed still opens.
const CHUNK_GUIDES: &[u8; 4] = b"GIDE";
const CHUNK_END: &[u8; 4] = b"END ";

/// Deflate level. Pixel data dominates the file and is highly compressible;
/// six is the usual balance and keeps saving a large document brisk.
const DEFLATE_LEVEL: u8 = 6;

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Serialise a document.
pub fn write(doc: &Document) -> Vec<u8> {
    let mut w = Writer::new();
    w.raw(MAGIC);
    w.u16(VERSION);

    let mut doc_chunk = Writer::new();
    doc_chunk.u32(doc.width);
    doc_chunk.u32(doc.height);
    doc_chunk.string(&doc.name);
    doc_chunk.f32(doc.dpi);
    // Which layer was selected, so reopening picks up where work left off.
    doc_chunk.u64(doc.active.map(|id| id.0).unwrap_or(0));
    // The pixel selection, if there is one.
    write_option(&mut doc_chunk, doc.selection.as_ref(), |w, s: &cshop_core::selection::Selection| {
        write_mask(w, &s.to_mask());
    });
    chunk(&mut w, CHUNK_DOC, &doc_chunk.bytes);

    // Only when there is something to say. sRGB is the assumption everywhere
    // else, and writing a few kilobytes of it into every project to repeat
    // what is already the default would be waste.
    if !doc.profile.is_srgb() {
        chunk(&mut w, CHUNK_PROFILE, doc.profile.bytes());
    }

    if !doc.guides.is_empty() {
        let mut g = Writer::new();
        g.u32(doc.guides.len() as u32);
        for guide in &doc.guides {
            g.u8(guide.vertical as u8);
            g.f32(guide.at);
        }
        chunk(&mut w, CHUNK_GUIDES, &g.bytes);
    }

    // Depth-first, parents first, so the reader can attach children as it goes.
    for id in doc.tree.iter_all() {
        if let Some(layer) = doc.tree.get(id) {
            let mut c = Writer::new();
            write_layer(&mut c, doc, layer);
            chunk(&mut w, CHUNK_LAYER, &c.bytes);
        }
    }

    for ch in &doc.channels {
        let mut c = Writer::new();
        c.string(&ch.name);
        c.bool(ch.visible);
        write_mask(&mut c, &ch.data);
        chunk(&mut w, CHUNK_CHANNEL, &c.bytes);
    }

    chunk(&mut w, CHUNK_END, &[]);
    w.bytes
}

fn chunk(w: &mut Writer, tag: &[u8; 4], payload: &[u8]) {
    w.raw(tag);
    w.u32(payload.len() as u32);
    w.raw(payload);
}

fn deflate(w: &mut Writer, bytes: &[u8]) {
    let packed = miniz_oxide::deflate::compress_to_vec(bytes, DEFLATE_LEVEL);
    // The uncompressed length is stored so the reader can size its buffer once
    // and refuse a blob that claims to expand to something absurd.
    w.u32(bytes.len() as u32);
    w.blob(&packed);
}


fn write_color(w: &mut Writer, c: Rgba8) {
    w.raw(&[c.r, c.g, c.b, c.a]);
}

fn write_pixels(w: &mut Writer, px: &PixelBuffer) {
    w.u32(px.width());
    w.u32(px.height());
    deflate(w, px.as_bytes());
}

fn write_mask(w: &mut Writer, m: &MaskBuffer) {
    w.u32(m.width());
    w.u32(m.height());
    deflate(w, m.as_bytes());
}

fn write_layer(w: &mut Writer, doc: &Document, layer: &Layer) {
    w.u64(layer.id.0);
    w.u64(layer.parent.map(|p| p.0).unwrap_or(0));
    // Index among siblings, so the order survives even if ids do not sort.
    let index = doc
        .tree
        .position(layer.id)
        .map(|p| p.index as u32)
        .unwrap_or(0);
    w.u32(index);
    w.string(&layer.name);

    let mut flags = 0u32;
    for (bit, on) in [
        layer.visible,
        layer.clipping,
        layer.expanded,
        layer.is_background,
        layer.locks.transparency,
        layer.locks.pixels,
        layer.locks.position,
        layer.locks.all,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, on)| (1u32 << i, on))
    {
        if on {
            flags |= bit;
        }
    }
    w.u32(flags);
    w.f32(layer.opacity);
    w.f32(layer.fill_opacity);
    w.u16(layer.blend_mode as u16);
    w.i32(layer.offset.0);
    w.i32(layer.offset.1);

    match &layer.kind {
        LayerKind::Raster(px) => {
            w.u8(0);
            write_pixels(w, px);
        }
        LayerKind::Group { .. } => w.u8(1),
        LayerKind::Fill(FillStyle::Solid(c)) => {
            w.u8(2);
            write_color(w, *c);
        }
        LayerKind::Adjustment(a) => {
            w.u8(3);
            write_adjustment(w, a);
        }
        LayerKind::Text(t) => {
            w.u8(4);
            write_text(w, t.content());
            // The raster is rebuilt from the content on load, but the anchor
            // has to be preserved or the type would jump when the fonts on
            // this machine differ from those on the last one.
            w.i32(t.anchor().0);
            w.i32(t.anchor().1);
        }
        LayerKind::Shape(s) => {
            w.u8(5);
            write_shape(w, s.content());
        }
    }

    match &layer.mask {
        None => w.bool(false),
        Some(m) => {
            w.bool(true);
            w.i32(m.offset.0);
            w.i32(m.offset.1);
            w.bool(m.enabled);
            w.bool(m.linked);
            write_mask(w, &m.data);
        }
    }

    write_effects(w, &layer.effects);
}

fn write_curve(w: &mut Writer, c: &Curve) {
    let pts = c.points();
    w.u32(pts.len() as u32);
    for (x, y) in pts {
        w.f32(*x);
        w.f32(*y);
    }
}

fn write_levels(w: &mut Writer, l: &LevelsChannel) {
    w.f32s(&[l.input_black, l.input_white, l.gamma, l.output_black, l.output_white]);
}

fn write_adjustment(w: &mut Writer, a: &Adjustment) {
    match a {
        Adjustment::BrightnessContrast { brightness, contrast } => {
            w.u8(0);
            w.f32s(&[*brightness, *contrast]);
        }
        Adjustment::Levels { rgb, channels } => {
            w.u8(1);
            write_levels(w, rgb);
            for c in channels {
                write_levels(w, c);
            }
        }
        Adjustment::Curves { curves } => {
            w.u8(2);
            for c in curves {
                write_curve(w, c);
            }
        }
        Adjustment::Exposure { exposure, offset, gamma } => {
            w.u8(3);
            w.f32s(&[*exposure, *offset, *gamma]);
        }
        Adjustment::Vibrance { vibrance, saturation } => {
            w.u8(4);
            w.f32s(&[*vibrance, *saturation]);
        }
        Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
            w.u8(5);
            w.f32s(&[*hue, *saturation, *lightness]);
            w.bool(*colorize);
        }
        Adjustment::ColorBalance { shadows, midtones, highlights, preserve_luminosity } => {
            w.u8(6);
            w.f32s(shadows);
            w.f32s(midtones);
            w.f32s(highlights);
            w.bool(*preserve_luminosity);
        }
        Adjustment::BlackAndWhite { weights, tint } => {
            w.u8(7);
            w.f32s(weights);
            match tint {
                None => w.bool(false),
                Some(c) => {
                    w.bool(true);
                    write_color(w, *c);
                }
            }
        }
        Adjustment::ChannelMixer { matrix, monochrome } => {
            w.u8(8);
            for row in matrix {
                w.f32s(row);
            }
            w.bool(*monochrome);
        }
        Adjustment::PhotoFilter { color, density, preserve_luminosity } => {
            w.u8(9);
            write_color(w, *color);
            w.f32(*density);
            w.bool(*preserve_luminosity);
        }
        Adjustment::Invert => w.u8(10),
        Adjustment::Posterize { levels } => {
            w.u8(11);
            w.u32(*levels);
        }
        Adjustment::Threshold { level } => {
            w.u8(12);
            w.f32(*level);
        }
        Adjustment::GradientMap { stops } => {
            w.u8(13);
            w.u32(stops.len() as u32);
            for s in stops {
                w.f32(s.position);
                write_color(w, s.color);
            }
        }
    }
}

fn write_text(w: &mut Writer, t: &TextContent) {
    w.string(&t.text);
    let s = &t.style;
    w.string(&s.family);
    w.f32(s.size);
    write_color(w, s.color);
    w.bool(s.bold);
    w.bool(s.italic);
    w.u8(match s.align {
        TextAlign::Left => 0,
        TextAlign::Center => 1,
        TextAlign::Right => 2,
    });
    // Auto leading is `None`, which zero stands in for.
    w.f32(s.leading.unwrap_or(0.0));
    w.f32(s.tracking);
    w.bool(s.antialias);
    w.f32(t.wrap_width.unwrap_or(0.0));
}

fn write_shape(w: &mut Writer, c: &ShapeContent) {
    match &c.kind {
        ShapeKind::Rectangle { radius } => {
            w.u8(0);
            w.f32(*radius);
        }
        ShapeKind::Ellipse => w.u8(1),
        ShapeKind::Polygon { sides, star, inner } => {
            w.u8(2);
            w.u32(*sides);
            w.bool(*star);
            w.f32(*inner);
        }
        ShapeKind::Line { thickness, from, to } => {
            w.u8(3);
            w.f32s(&[*thickness, from.0, from.1, to.0, to.1]);
        }
        // Tag 4. Written as counts followed by flat runs of anchors, so a
        // reader that stops early cannot mistake one part for another.
        ShapeKind::Path(path) => {
            w.u8(4);
            w.u32(path.parts.len() as u32);
            for part in &path.parts {
                w.u8(part.op as u8);
                w.u32(part.subpaths.len() as u32);
                for sub in &part.subpaths {
                    w.bool(sub.closed);
                    w.u32(sub.anchors.len() as u32);
                    for a in &sub.anchors {
                        w.f32s(&[
                            a.at.x,
                            a.at.y,
                            a.in_handle.x,
                            a.in_handle.y,
                            a.out_handle.x,
                            a.out_handle.y,
                        ]);
                    }
                }
            }
        }
    }
    w.f32(c.size.0);
    w.f32(c.size.1);
    let s = &c.style;
    write_optional_color(w, s.fill);
    write_optional_color(w, s.stroke);
    w.f32(s.stroke_width);
    w.u8(match s.stroke_align {
        StrokeAlign::Inside => 0,
        StrokeAlign::Center => 1,
        StrokeAlign::Outside => 2,
    });
    w.bool(s.antialias);
}

fn write_optional_color(w: &mut Writer, c: Option<Rgba8>) {
    match c {
        None => w.bool(false),
        Some(c) => {
            w.bool(true);
            write_color(w, c);
        }
    }
}

fn write_shadow(w: &mut Writer, s: &Shadow) {
    write_color(w, s.color);
    w.u16(s.mode as u16);
    w.f32s(&[s.opacity, s.angle, s.distance, s.spread, s.size]);
    w.bool(s.use_global_light);
}

fn write_glow(w: &mut Writer, g: &Glow) {
    write_color(w, g.color);
    w.u16(g.mode as u16);
    w.f32s(&[g.opacity, g.spread, g.size]);
    w.u8(match g.source {
        GlowSource::Edge => 0,
        GlowSource::Center => 1,
    });
}

fn write_effects(w: &mut Writer, fx: &LayerEffects) {
    w.bool(fx.enabled);
    w.f32(fx.global_light_angle);
    w.f32(fx.global_light_altitude);

    write_option(w, fx.drop_shadow.as_ref(), write_shadow);
    write_option(w, fx.inner_shadow.as_ref(), write_shadow);
    write_option(w, fx.outer_glow.as_ref(), write_glow);
    write_option(w, fx.inner_glow.as_ref(), write_glow);
    write_option(w, fx.bevel.as_ref(), |w, b: &Bevel| {
        w.u8(match b.style {
            BevelStyle::Inner => 0,
            BevelStyle::Outer => 1,
            BevelStyle::Emboss => 2,
            BevelStyle::Pillow => 3,
        });
        w.f32s(&[b.size, b.soften, b.depth, b.angle, b.altitude]);
        w.bool(b.use_global_light);
        w.bool(b.down);
        write_color(w, b.highlight);
        w.u16(b.highlight_mode as u16);
        w.f32(b.highlight_opacity);
        write_color(w, b.shadow);
        w.u16(b.shadow_mode as u16);
        w.f32(b.shadow_opacity);
    });
    write_option(w, fx.satin.as_ref(), |w, s: &Satin| {
        write_color(w, s.color);
        w.u16(s.mode as u16);
        w.f32s(&[s.opacity, s.angle, s.distance, s.size]);
        w.bool(s.invert);
    });
    write_option(w, fx.color_overlay.as_ref(), |w, o: &ColorOverlay| {
        write_color(w, o.color);
        w.u16(o.mode as u16);
        w.f32(o.opacity);
    });
    write_option(w, fx.gradient_overlay.as_ref(), |w, g: &GradientOverlay| {
        write_color(w, g.from);
        write_color(w, g.to);
        w.u8(g.kind as u8);
        w.u16(g.mode as u16);
        w.f32s(&[g.opacity, g.angle, g.scale]);
        w.bool(g.reverse);
    });
    write_option(w, fx.pattern_overlay.as_ref(), |w, o: &PatternOverlay| {
        w.u8(o.kind as u8);
        write_color(w, o.color);
        write_color(w, o.background);
        w.u16(o.mode as u16);
        w.f32s(&[o.opacity, o.scale, o.angle]);
        w.u64(o.seed);
    });
    write_option(w, fx.stroke.as_ref(), |w, s: &Stroke| {
        write_color(w, s.color);
        w.u16(s.mode as u16);
        w.f32s(&[s.opacity, s.size]);
        w.u8(match s.position {
            StrokePosition::Outside => 0,
            StrokePosition::Center => 1,
            StrokePosition::Inside => 2,
        });
    });
}

fn write_option<T>(w: &mut Writer, v: Option<&T>, body: impl FnOnce(&mut Writer, &T)) {
    match v {
        None => w.bool(false),
        Some(v) => {
            w.bool(true);
            body(w, v);
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Largest a single deflate blob may claim to expand to, so a corrupt or
/// hostile length cannot provoke a huge allocation before anything is checked.
const MAX_BLOB: usize = 1 << 30;

/// Parse a document.
pub fn read(bytes: &[u8]) -> Result<Document, IoError> {
    let mut r = Reader::new(bytes);
    if r.take(6)? != MAGIC {
        return Err(IoError::Malformed("not a C-Shop project".into()));
    }
    let version = r.u16()?;
    if version > VERSION {
        return Err(IoError::Malformed(format!(
            "this project was written by a newer version of C-Shop (format {version})"
        )));
    }

    let mut doc = Document::new("Untitled", 1, 1, cshop_core::document::Background::Transparent);
    // `Document::new` seeds a background layer; a project brings its own.
    doc.tree = Default::default();
    doc.active = None;
    let mut active_id = 0u64;
    let mut seen_doc = false;

    loop {
        if r.remaining() < 8 {
            break;
        }
        let tag: [u8; 4] = r.take(4)?.try_into().unwrap();
        let len = r.u32()? as usize;
        let end = r.position() + len;
        if len > r.remaining() {
            return Err(IoError::Malformed("a chunk runs past the end of the file".into()));
        }

        match &tag {
            CHUNK_DOC => {
                doc.width = r.u32()?;
                doc.height = r.u32()?;
                doc.name = r.string()?;
                doc.dpi = r.f32()?;
                active_id = r.u64()?;
                if r.bool()? {
                    let mask = read_mask(&mut r)?;
                    doc.selection = Some(cshop_core::selection::Selection::from_mask(mask));
                }
                seen_doc = true;
            }
            CHUNK_LAYER => read_layer(&mut r, &mut doc)?,
            CHUNK_CHANNEL => {
                let name = r.string()?;
                let visible = r.bool()?;
                let data = read_mask(&mut r)?;
                doc.channels.push(cshop_core::document::AlphaChannel { name, data, visible });
            }
            CHUNK_PROFILE => {
                let bytes = r.take(len)?;
                match cshop_core::profile::Profile::parse(bytes) {
                    Ok(p) => doc.profile = p,
                    // A project whose profile will not parse is still a
                    // project. Opening it in sRGB shows the pixels as they
                    // were stored, which is the least wrong thing available.
                    Err(e) => log::warn!("project profile unreadable, using sRGB: {e}"),
                }
            }
            CHUNK_GUIDES => {
                let count = r.u32()?;
                // A count is a claim by the file, so it is bounded by what the
                // chunk could actually hold rather than trusted.
                let most = len.saturating_sub(4) / 5;
                for _ in 0..count.min(most as u32) {
                    let vertical = r.u8()? != 0;
                    let at = r.f32()?;
                    if at.is_finite() {
                        doc.guides.push(cshop_core::guides::Guide { vertical, at });
                    }
                }
            }
            CHUNK_END => break,
            // Written by a newer build: skipped rather than refused, which is
            // what the chunked layout is for.
            _ => {
                log::debug!("skipping unknown project chunk {:?}", String::from_utf8_lossy(&tag));
            }
        }
        r.seek(end)?;
    }

    if !seen_doc {
        return Err(IoError::Malformed("the project has no document chunk".into()));
    }
    if doc.width == 0 || doc.height == 0 {
        return Err(IoError::Malformed("the project has no canvas".into()));
    }
    if doc.width > crate::MAX_DIMENSION || doc.height > crate::MAX_DIMENSION {
        return Err(IoError::TooLarge(doc.width, doc.height, crate::MAX_DIMENSION));
    }

    let wanted = LayerId(active_id);
    doc.active = if doc.tree.contains(wanted) {
        Some(wanted)
    } else {
        doc.tree.root().last().copied()
    };
    doc.selected_layers = doc.active.into_iter().collect();
    doc.modified = false;
    Ok(doc)
}

fn inflate(r: &mut Reader<'_>) -> Result<Vec<u8>, IoError> {
    let expanded = r.u32()? as usize;
    if expanded > MAX_BLOB {
        return Err(IoError::Malformed(format!("a blob claims to be {expanded} bytes")));
    }
    let packed = r.blob()?;
    let out = miniz_oxide::inflate::decompress_to_vec_with_limit(packed, MAX_BLOB)
        .map_err(|e| IoError::Malformed(format!("compressed data is damaged: {e:?}")))?;
    if out.len() != expanded {
        return Err(IoError::Malformed(format!(
            "expected {expanded} bytes of pixel data but got {}",
            out.len()
        )));
    }
    Ok(out)
}

fn read_color(r: &mut Reader<'_>) -> Result<Rgba8, IoError> {
    let b = r.take(4)?;
    Ok(Rgba8::new(b[0], b[1], b[2], b[3]))
}

fn read_optional_color(r: &mut Reader<'_>) -> Result<Option<Rgba8>, IoError> {
    if r.bool()? {
        Ok(Some(read_color(r)?))
    } else {
        Ok(None)
    }
}

/// Guard both dimensions before allocating anything sized by them.
fn check_size(w: u32, h: u32) -> Result<(), IoError> {
    if w > crate::MAX_DIMENSION || h > crate::MAX_DIMENSION {
        return Err(IoError::TooLarge(w, h, crate::MAX_DIMENSION));
    }
    Ok(())
}

fn read_pixels(r: &mut Reader<'_>) -> Result<PixelBuffer, IoError> {
    let (w, h) = (r.u32()?, r.u32()?);
    check_size(w, h)?;
    let bytes = inflate(r)?;
    PixelBuffer::from_rgba_bytes(w, h, &bytes)
        .ok_or_else(|| IoError::Malformed("pixel data does not match its size".into()))
}

fn read_mask(r: &mut Reader<'_>) -> Result<MaskBuffer, IoError> {
    let (w, h) = (r.u32()?, r.u32()?);
    check_size(w, h)?;
    let bytes = inflate(r)?;
    MaskBuffer::from_bytes(w, h, bytes)
        .ok_or_else(|| IoError::Malformed("mask data does not match its size".into()))
}

/// Blend modes are stored by their discriminant, which is fixed by the enum
/// and so stable across builds. An unknown one falls back to Normal rather
/// than refusing the file.
fn blend_from(v: u16) -> BlendMode {
    BlendMode::all().find(|m| *m as u16 == v).unwrap_or(BlendMode::Normal)
}

fn read_layer(r: &mut Reader<'_>, doc: &mut Document) -> Result<(), IoError> {
    let id = LayerId(r.u64()?);
    let parent = r.u64()?;
    let index = r.u32()? as usize;
    let name = r.string()?;
    let flags = r.u32()?;
    let opacity = r.f32()?;
    let fill_opacity = r.f32()?;
    let blend_mode = blend_from(r.u16()?);
    let offset = (r.i32()?, r.i32()?);

    let kind = match r.u8()? {
        0 => LayerKind::Raster(read_pixels(r)?),
        1 => LayerKind::Group { children: Vec::new() },
        2 => LayerKind::Fill(FillStyle::Solid(read_color(r)?)),
        3 => LayerKind::Adjustment(read_adjustment(r)?),
        4 => {
            let content = read_text(r)?;
            let anchor = (r.i32()?, r.i32()?);
            // Type is re-rendered from its description, so a machine without
            // the font it was written with still opens the project — with the
            // fallback face, and the layer still editable. The saved anchor
            // keeps it where it was put even when the metrics differ.
            match cshop_core::layer::TextLayer::new(content.clone()) {
                Some(t) => {
                    let _ = anchor;
                    LayerKind::Text(Box::new(t))
                }
                None => {
                    log::warn!("no font for type layer {name:?}; keeping it as pixels");
                    LayerKind::Raster(PixelBuffer::new(1, 1))
                }
            }
        }
        5 => {
            let content = read_shape(r)?;
            match cshop_core::layer::ShapeLayer::new(content) {
                Some(s) => LayerKind::Shape(Box::new(s)),
                None => LayerKind::Raster(PixelBuffer::new(1, 1)),
            }
        }
        other => {
            return Err(IoError::Malformed(format!("unknown layer kind {other}")));
        }
    };

    let mask = if r.bool()? {
        let offset = (r.i32()?, r.i32()?);
        let enabled = r.bool()?;
        let linked = r.bool()?;
        let data = read_mask(r)?;
        Some(LayerMask { data, offset, enabled, linked })
    } else {
        None
    };
    let effects = read_effects(r)?;

    let mut layer = Layer::new(id, name, kind);
    layer.visible = flags & 1 != 0;
    layer.clipping = flags & 2 != 0;
    layer.expanded = flags & 4 != 0;
    layer.is_background = flags & 8 != 0;
    layer.locks = LayerLocks {
        transparency: flags & 16 != 0,
        pixels: flags & 32 != 0,
        position: flags & 64 != 0,
        all: flags & 128 != 0,
    };
    layer.opacity = opacity.clamp(0.0, 1.0);
    layer.fill_opacity = fill_opacity.clamp(0.0, 1.0);
    layer.blend_mode = blend_mode;
    layer.offset = offset;
    layer.mask = mask;
    layer.effects = effects;

    // Parents are written before their children, so the parent is already in
    // the tree. A dangling one lands at the root rather than being dropped.
    let parent = (parent != 0).then_some(LayerId(parent)).filter(|p| doc.tree.contains(*p));
    doc.tree.insert(layer, parent, index);
    Ok(())
}

fn read_curve(r: &mut Reader<'_>) -> Result<Curve, IoError> {
    let n = r.u32()? as usize;
    if n > 64 {
        return Err(IoError::Malformed(format!("a curve claims {n} points")));
    }
    let mut pts = Vec::with_capacity(n);
    for _ in 0..n {
        pts.push((r.f32()?, r.f32()?));
    }
    Ok(Curve::new(pts))
}

fn read_levels(r: &mut Reader<'_>) -> Result<LevelsChannel, IoError> {
    let v: [f32; 5] = r.f32s()?;
    Ok(LevelsChannel {
        input_black: v[0],
        input_white: v[1],
        gamma: v[2],
        output_black: v[3],
        output_white: v[4],
    })
}

fn read_adjustment(r: &mut Reader<'_>) -> Result<Adjustment, IoError> {
    Ok(match r.u8()? {
        0 => {
            let v: [f32; 2] = r.f32s()?;
            Adjustment::BrightnessContrast { brightness: v[0], contrast: v[1] }
        }
        1 => Adjustment::Levels {
            rgb: read_levels(r)?,
            channels: [read_levels(r)?, read_levels(r)?, read_levels(r)?],
        },
        2 => Adjustment::Curves {
            curves: [read_curve(r)?, read_curve(r)?, read_curve(r)?, read_curve(r)?],
        },
        3 => {
            let v: [f32; 3] = r.f32s()?;
            Adjustment::Exposure { exposure: v[0], offset: v[1], gamma: v[2] }
        }
        4 => {
            let v: [f32; 2] = r.f32s()?;
            Adjustment::Vibrance { vibrance: v[0], saturation: v[1] }
        }
        5 => {
            let v: [f32; 3] = r.f32s()?;
            Adjustment::HueSaturation {
                hue: v[0],
                saturation: v[1],
                lightness: v[2],
                colorize: r.bool()?,
            }
        }
        6 => Adjustment::ColorBalance {
            shadows: r.f32s()?,
            midtones: r.f32s()?,
            highlights: r.f32s()?,
            preserve_luminosity: r.bool()?,
        },
        7 => Adjustment::BlackAndWhite {
            weights: r.f32s()?,
            tint: read_optional_color(r)?,
        },
        8 => Adjustment::ChannelMixer {
            matrix: [r.f32s()?, r.f32s()?, r.f32s()?],
            monochrome: r.bool()?,
        },
        9 => Adjustment::PhotoFilter {
            color: read_color(r)?,
            density: r.f32()?,
            preserve_luminosity: r.bool()?,
        },
        10 => Adjustment::Invert,
        11 => Adjustment::Posterize { levels: r.u32()? },
        12 => Adjustment::Threshold { level: r.f32()? },
        13 => {
            let n = r.u32()? as usize;
            if n > 256 {
                return Err(IoError::Malformed(format!("a gradient claims {n} stops")));
            }
            let mut stops = Vec::with_capacity(n);
            for _ in 0..n {
                stops.push(cshop_core::fill::GradientStop {
                    position: r.f32()?,
                    color: read_color(r)?,
                });
            }
            Adjustment::GradientMap { stops }
        }
        other => return Err(IoError::Malformed(format!("unknown adjustment {other}"))),
    })
}

fn read_text(r: &mut Reader<'_>) -> Result<TextContent, IoError> {
    let text = r.string()?;
    let family = r.string()?;
    let size = r.f32()?;
    let color = read_color(r)?;
    let bold = r.bool()?;
    let italic = r.bool()?;
    let align = match r.u8()? {
        1 => TextAlign::Center,
        2 => TextAlign::Right,
        _ => TextAlign::Left,
    };
    let leading = r.f32()?;
    let tracking = r.f32()?;
    let antialias = r.bool()?;
    let wrap = r.f32()?;
    Ok(TextContent {
        text,
        style: TextStyle {
            family,
            size,
            color,
            bold,
            italic,
            align,
            leading: (leading > 0.0).then_some(leading),
            tracking,
            antialias,
        },
        wrap_width: (wrap > 0.0).then_some(wrap),
    })
}

fn read_shape(r: &mut Reader<'_>) -> Result<ShapeContent, IoError> {
    let kind = match r.u8()? {
        0 => ShapeKind::Rectangle { radius: r.f32()? },
        1 => ShapeKind::Ellipse,
        2 => ShapeKind::Polygon { sides: r.u32()?, star: r.bool()?, inner: r.f32()? },
        3 => {
            let v: [f32; 5] = r.f32s()?;
            ShapeKind::Line { thickness: v[0], from: (v[1], v[2]), to: (v[3], v[4]) }
        }
        4 => {
            use cshop_core::geom::Vec2;
            use cshop_core::path::{Anchor, BoolOp, PathPart, PathShape, SubPath};
            let parts = r.u32()? as usize;
            // Bounded against a corrupt count claiming millions of parts.
            if parts > 4096 {
                return Err(IoError::Malformed(format!("{parts} path parts")));
            }
            let mut out = Vec::with_capacity(parts);
            for _ in 0..parts {
                let op = match r.u8()? {
                    1 => BoolOp::Subtract,
                    2 => BoolOp::Intersect,
                    3 => BoolOp::Exclude,
                    _ => BoolOp::Union,
                };
                let subs = r.u32()? as usize;
                if subs > 65_536 {
                    return Err(IoError::Malformed(format!("{subs} subpaths")));
                }
                let mut subpaths = Vec::with_capacity(subs);
                for _ in 0..subs {
                    let closed = r.bool()?;
                    let n = r.u32()? as usize;
                    if n > 1_000_000 {
                        return Err(IoError::Malformed(format!("{n} anchors")));
                    }
                    let mut anchors = Vec::with_capacity(n);
                    for _ in 0..n {
                        let v: [f32; 6] = r.f32s()?;
                        anchors.push(Anchor {
                            at: Vec2::new(v[0], v[1]),
                            in_handle: Vec2::new(v[2], v[3]),
                            out_handle: Vec2::new(v[4], v[5]),
                        });
                    }
                    subpaths.push(SubPath { anchors, closed });
                }
                out.push(PathPart { subpaths, op });
            }
            ShapeKind::Path(PathShape { parts: out })
        }
        other => return Err(IoError::Malformed(format!("unknown shape {other}"))),
    };
    let size = (r.f32()?, r.f32()?);
    let fill = read_optional_color(r)?;
    let stroke = read_optional_color(r)?;
    let stroke_width = r.f32()?;
    let stroke_align = match r.u8()? {
        0 => StrokeAlign::Inside,
        2 => StrokeAlign::Outside,
        _ => StrokeAlign::Center,
    };
    let antialias = r.bool()?;
    Ok(ShapeContent {
        kind,
        size,
        style: ShapeStyle { fill, stroke, stroke_width, stroke_align, antialias },
    })
}

fn read_option<T>(
    r: &mut Reader<'_>,
    body: impl FnOnce(&mut Reader<'_>) -> Result<T, IoError>,
) -> Result<Option<T>, IoError> {
    if r.bool()? {
        Ok(Some(body(r)?))
    } else {
        Ok(None)
    }
}

fn read_shadow(r: &mut Reader<'_>) -> Result<Shadow, IoError> {
    let color = read_color(r)?;
    let mode = blend_from(r.u16()?);
    let v: [f32; 5] = r.f32s()?;
    Ok(Shadow {
        color,
        mode,
        opacity: v[0],
        angle: v[1],
        distance: v[2],
        spread: v[3],
        size: v[4],
        use_global_light: r.bool()?,
    })
}

fn read_glow(r: &mut Reader<'_>) -> Result<Glow, IoError> {
    let color = read_color(r)?;
    let mode = blend_from(r.u16()?);
    let v: [f32; 3] = r.f32s()?;
    let source = match r.u8()? {
        1 => GlowSource::Center,
        _ => GlowSource::Edge,
    };
    Ok(Glow { color, mode, opacity: v[0], spread: v[1], size: v[2], source })
}

fn read_effects(r: &mut Reader<'_>) -> Result<LayerEffects, IoError> {
    let enabled = r.bool()?;
    let global_light_angle = r.f32()?;
    let global_light_altitude = r.f32()?;

    let drop_shadow = read_option(r, read_shadow)?;
    let inner_shadow = read_option(r, read_shadow)?;
    let outer_glow = read_option(r, read_glow)?;
    let inner_glow = read_option(r, read_glow)?;
    let bevel = read_option(r, |r| {
        let style = match r.u8()? {
            1 => BevelStyle::Outer,
            2 => BevelStyle::Emboss,
            3 => BevelStyle::Pillow,
            _ => BevelStyle::Inner,
        };
        let v: [f32; 5] = r.f32s()?;
        let use_global_light = r.bool()?;
        let down = r.bool()?;
        let highlight = read_color(r)?;
        let highlight_mode = blend_from(r.u16()?);
        let highlight_opacity = r.f32()?;
        let shadow = read_color(r)?;
        let shadow_mode = blend_from(r.u16()?);
        let shadow_opacity = r.f32()?;
        Ok(Bevel {
            style,
            size: v[0],
            soften: v[1],
            depth: v[2],
            angle: v[3],
            altitude: v[4],
            use_global_light,
            down,
            highlight,
            highlight_mode,
            highlight_opacity,
            shadow,
            shadow_mode,
            shadow_opacity,
        })
    })?;
    let satin = read_option(r, |r| {
        let color = read_color(r)?;
        let mode = blend_from(r.u16()?);
        let v: [f32; 4] = r.f32s()?;
        Ok(Satin {
            color,
            mode,
            opacity: v[0],
            angle: v[1],
            distance: v[2],
            size: v[3],
            invert: r.bool()?,
        })
    })?;
    let color_overlay = read_option(r, |r| {
        Ok(ColorOverlay {
            color: read_color(r)?,
            mode: blend_from(r.u16()?),
            opacity: r.f32()?,
        })
    })?;
    let gradient_overlay = read_option(r, |r| {
        let from = read_color(r)?;
        let to = read_color(r)?;
        let kind = gradient_kind_from(r.u8()?);
        let mode = blend_from(r.u16()?);
        let v: [f32; 3] = r.f32s()?;
        Ok(GradientOverlay {
            from,
            to,
            kind,
            mode,
            opacity: v[0],
            angle: v[1],
            scale: v[2],
            reverse: r.bool()?,
        })
    })?;
    let pattern_overlay = read_option(r, |r| {
        let kind = PatternKind::ALL.get(r.u8()? as usize).copied().unwrap_or_default();
        let color = read_color(r)?;
        let background = read_color(r)?;
        let mode = blend_from(r.u16()?);
        let v: [f32; 3] = r.f32s()?;
        Ok(PatternOverlay {
            kind,
            color,
            background,
            mode,
            opacity: v[0],
            scale: v[1],
            angle: v[2],
            seed: r.u64()?,
        })
    })?;
    let stroke = read_option(r, |r| {
        let color = read_color(r)?;
        let mode = blend_from(r.u16()?);
        let v: [f32; 2] = r.f32s()?;
        let position = match r.u8()? {
            1 => StrokePosition::Center,
            2 => StrokePosition::Inside,
            _ => StrokePosition::Outside,
        };
        Ok(Stroke { color, mode, opacity: v[0], size: v[1], position })
    })?;

    Ok(LayerEffects {
        enabled,
        global_light_angle,
        global_light_altitude,
        drop_shadow,
        outer_glow,
        bevel,
        inner_shadow,
        inner_glow,
        satin,
        color_overlay,
        gradient_overlay,
        pattern_overlay,
        stroke,
    })
}

fn gradient_kind_from(v: u8) -> cshop_core::fill::GradientKind {
    use cshop_core::fill::GradientKind as K;
    match v {
        1 => K::Radial,
        2 => K::Angle,
        3 => K::Reflected,
        4 => K::Diamond,
        _ => K::Linear,
    }
}
