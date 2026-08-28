//! PSD reading and writing.
//!
//! Enough of the format to move layered work in and out: RGB, 8 bits per
//! channel, layers with their names, bounds, opacity, blend mode, visibility,
//! clipping and layer masks, groups, and the flattened composite that other
//! programs show when they do not read layers.
//!
//! Everything here is **big-endian**, which is the format's convention.
//!
//! # What a group looks like in the file
//!
//! There is no nesting in the layer list. Layers are stored bottom to top, and
//! a group is marked by two extra entries: a *bounding* divider below its
//! children and a header entry above them carrying the group's name. Reading
//! walks bottom to top, opening a scope at the bounding divider and closing it
//! at the header; writing emits the same pair around each group's children.

use crate::bytes::{Reader, Writer};
use crate::IoError;
use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_core::layer::{Layer, LayerId, LayerMask};
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;

const SIGNATURE: &[u8; 4] = b"8BPS";
const RESOURCE_TAG: &[u8; 4] = b"8BIM";

/// PSD blend keys, paired with ours. The ones with no counterpart here read
/// back as Normal rather than being refused.
const BLEND_KEYS: &[(&[u8; 4], BlendMode)] = &[
    (b"norm", BlendMode::Normal),
    (b"diss", BlendMode::Dissolve),
    (b"dark", BlendMode::Darken),
    (b"mul ", BlendMode::Multiply),
    (b"idiv", BlendMode::ColorBurn),
    (b"lbrn", BlendMode::LinearBurn),
    (b"dkCl", BlendMode::DarkerColor),
    (b"lite", BlendMode::Lighten),
    (b"scrn", BlendMode::Screen),
    (b"div ", BlendMode::ColorDodge),
    (b"lddg", BlendMode::LinearDodge),
    (b"lgCl", BlendMode::LighterColor),
    (b"over", BlendMode::Overlay),
    (b"sLit", BlendMode::SoftLight),
    (b"hLit", BlendMode::HardLight),
    (b"vLit", BlendMode::VividLight),
    (b"lLit", BlendMode::LinearLight),
    (b"pLit", BlendMode::PinLight),
    (b"hMix", BlendMode::HardMix),
    (b"diff", BlendMode::Difference),
    (b"smud", BlendMode::Exclusion),
    (b"fsub", BlendMode::Subtract),
    (b"fdiv", BlendMode::Divide),
    (b"hue ", BlendMode::Hue),
    (b"sat ", BlendMode::Saturation),
    (b"colr", BlendMode::Color),
    (b"lum ", BlendMode::Luminosity),
    (b"pass", BlendMode::PassThrough),
];

fn blend_to_key(mode: BlendMode) -> &'static [u8; 4] {
    BLEND_KEYS.iter().find(|(_, m)| *m == mode).map(|(k, _)| *k).unwrap_or(b"norm")
}

fn blend_from_key(key: &[u8]) -> BlendMode {
    BLEND_KEYS
        .iter()
        .find(|(k, _)| k.as_slice() == key)
        .map(|(_, m)| *m)
        .unwrap_or(BlendMode::Normal)
}

// ---------------------------------------------------------------------------
// PackBits
// ---------------------------------------------------------------------------

/// PackBits run-length encoding, which is what PSD calls compression 1.
///
/// A header byte of `0..=127` means the next `n + 1` bytes are literal; one of
/// `-1..=-127` means the next byte repeats `1 - n` times; `-128` is skipped.
fn pack_bits(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + src.len() / 64 + 2);
    let mut i = 0;
    while i < src.len() {
        // How far the run of equal bytes at `i` reaches, capped at 128.
        let mut run = 1;
        while i + run < src.len() && src[i + run] == src[i] && run < 128 {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(src[i]);
            i += run;
        } else {
            // Gather literals until a run of three would pay for a new header.
            let start = i;
            while i < src.len() && i - start < 128 {
                let same = i + 2 < src.len() && src[i] == src[i + 1] && src[i] == src[i + 2];
                if same {
                    break;
                }
                i += 1;
            }
            let n = i - start;
            out.push((n - 1) as u8);
            out.extend_from_slice(&src[start..i]);
        }
    }
    out
}

fn unpack_bits(r: &mut Reader<'_>, expected: usize) -> Result<Vec<u8>, IoError> {
    let mut out = Vec::with_capacity(expected);
    while out.len() < expected {
        let header = r.u8()? as i8;
        if header == -128 {
            continue;
        }
        if header >= 0 {
            let n = header as usize + 1;
            out.extend_from_slice(r.take(n)?);
        } else {
            let n = 1 - header as isize;
            let b = r.u8()?;
            out.extend(std::iter::repeat_n(b, n as usize));
        }
    }
    out.truncate(expected);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// One entry in the flat list a PSD stores, which is not the same shape as a
/// layer tree: groups become a pair of markers around their children.
enum Entry<'a> {
    /// A real layer with pixels.
    Layer(&'a Layer),
    /// The marker below a group's children.
    GroupStart,
    /// The marker above them, carrying the group's name and settings.
    GroupEnd(&'a Layer),
}

/// Flatten the tree into the bottom-to-top list the format wants.
fn flatten<'a>(doc: &'a Document, parent: Option<LayerId>, out: &mut Vec<Entry<'a>>) {
    for id in doc.tree.children(parent) {
        let Some(layer) = doc.tree.get(*id) else { continue };
        if layer.kind.is_group() {
            out.push(Entry::GroupStart);
            flatten(doc, Some(*id), out);
            out.push(Entry::GroupEnd(layer));
        } else {
            out.push(Entry::Layer(layer));
        }
    }
}

/// A Pascal string padded so the whole field is a multiple of `align`.
fn pascal_string(w: &mut Writer, s: &str, align: usize) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(255);
    w.u8(n as u8);
    w.raw(&bytes[..n]);
    let written = 1 + n;
    let pad = (align - written % align) % align;
    w.raw(&vec![0u8; pad]);
}

/// The four channels a layer contributes, as planar bytes over `rect`.
fn layer_channels(layer: &Layer, rect: IRect) -> Vec<Vec<u8>> {
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let mut planes = vec![vec![0u8; w * h]; 4];
    if let Some(px) = layer.pixels() {
        let (ox, oy) = layer.offset;
        for y in 0..h {
            for x in 0..w {
                let p = px.get(rect.x0 + x as i32 - ox, rect.y0 + y as i32 - oy);
                let i = y * w + x;
                planes[0][i] = p.a;
                planes[1][i] = p.r;
                planes[2][i] = p.g;
                planes[3][i] = p.b;
            }
        }
    }
    planes
}

/// Compress one plane, returning the packed bytes and the per-row byte counts
/// the format stores ahead of them.
fn rle_plane(plane: &[u8], width: usize, height: usize) -> (Vec<u16>, Vec<u8>) {
    let mut counts = Vec::with_capacity(height);
    let mut data = Vec::new();
    for y in 0..height {
        let row = &plane[y * width..(y + 1) * width];
        let packed = pack_bits(row);
        counts.push(packed.len() as u16);
        data.extend_from_slice(&packed);
    }
    (counts, data)
}

/// Serialise a document as a PSD.
pub fn write(doc: &Document, composite: &PixelBuffer) -> Result<Vec<u8>, IoError> {
    if doc.width == 0 || doc.height == 0 {
        return Err(IoError::Malformed("cannot write an empty canvas".into()));
    }
    // The format's own ceiling for version 1.
    if doc.width > 30_000 || doc.height > 30_000 {
        return Err(IoError::Unsupported(
            "PSD holds at most 30000 pixels per side; use PSB or a smaller canvas".into(),
        ));
    }

    let mut w = Writer::new();
    w.raw(SIGNATURE);
    w.be_u16(1);
    w.raw(&[0u8; 6]);
    w.be_u16(4); // RGBA
    w.be_u32(doc.height);
    w.be_u32(doc.width);
    w.be_u16(8);
    w.be_u16(3); // RGB colour mode

    w.be_u32(0); // colour mode data
    w.be_u32(0); // image resources

    // --- layer and mask information ---------------------------------------
    let mut entries = Vec::new();
    flatten(doc, None, &mut entries);

    let mut records = Writer::new();
    let mut channel_data = Writer::new();
    records.be_i16(-(entries.len() as i16)); // negative: the first channel is alpha

    for entry in &entries {
        let (layer, rect, is_divider) = match entry {
            Entry::Layer(l) => {
                // A layer with no pixels of its own still needs a rect.
                let b = l.bounds();
                (Some(*l), if b.is_empty() { IRect::at(0, 0, 1, 1) } else { b }, false)
            }
            Entry::GroupEnd(l) => (Some(*l), IRect::at(0, 0, 0, 0), true),
            Entry::GroupStart => (None, IRect::at(0, 0, 0, 0), true),
        };

        records.be_i32(rect.y0);
        records.be_i32(rect.x0);
        records.be_i32(rect.y1);
        records.be_i32(rect.x1);

        let planes = match entry {
            Entry::Layer(l) => layer_channels(l, rect),
            // The markers carry no image data, but the format still expects
            // four zero-sized channels.
            _ => vec![Vec::new(); 4],
        };
        let (pw, ph) = (rect.width() as usize, rect.height() as usize);

        // The mask, when there is one, is a fifth channel with its own size.
        let mask_plane: Option<(Vec<u8>, usize, usize)> = layer
            .and_then(|l| l.mask.as_ref())
            .map(|m| {
                (
                    m.data.as_bytes().to_vec(),
                    m.data.width() as usize,
                    m.data.height() as usize,
                )
            })
            .filter(|(_, w, h)| *w > 0 && *h > 0);

        records.be_u16(4 + mask_plane.is_some() as u16);
        // Channel ids: -1 is alpha, then red, green, blue, then -2 for a mask.
        let mut packed_channels: Vec<(Vec<u16>, Vec<u8>)> = Vec::new();
        let declare = |records: &mut Writer,
                           id: i16,
                           plane: &[u8],
                           w: usize,
                           h: usize,
                           packed: &mut Vec<(Vec<u16>, Vec<u8>)>| {
            let (counts, data) =
                if plane.is_empty() { (Vec::new(), Vec::new()) } else { rle_plane(plane, w, h) };
            // Two bytes for the compression tag, then the row counts, then the
            // data — and the same two bytes even when there is no data at all,
            // or the declared length would not match what is written.
            let len = 2 + counts.len() * 2 + data.len();
            records.be_i16(id);
            records.be_u32(len as u32);
            packed.push((counts, data));
        };
        for (i, plane) in planes.iter().enumerate() {
            let id: i16 = if i == 0 { -1 } else { i as i16 - 1 };
            declare(&mut records, id, plane, pw, ph, &mut packed_channels);
        }
        if let Some((plane, mw, mh)) = &mask_plane {
            declare(&mut records, -2, plane, *mw, *mh, &mut packed_channels);
        }

        records.raw(RESOURCE_TAG);
        records.raw(blend_to_key(layer.map(|l| l.blend_mode).unwrap_or(BlendMode::Normal)));
        records.u8((layer.map(|l| l.opacity).unwrap_or(1.0) * 255.0).round() as u8);
        records.u8(layer.map(|l| l.clipping as u8).unwrap_or(0));
        // Bit 1 is *hidden*, not visible.
        let hidden = layer.map(|l| !l.visible).unwrap_or(false);
        let mut flags = 0u8;
        if hidden {
            flags |= 0x02;
        }
        if layer.map(|l| l.locks.transparency).unwrap_or(false) {
            flags |= 0x01;
        }
        records.u8(flags);
        records.u8(0); // filler

        // --- extra data ---
        let mut extra = Writer::new();
        write_layer_mask(&mut extra, layer);
        extra.be_u32(0); // blending ranges
        pascal_string(&mut extra, layer.map(|l| l.name.as_str()).unwrap_or("</Layer group>"), 4);

        // Unicode name, which is what modern readers actually use.
        if let Some(l) = layer {
            additional(&mut extra, b"luni", |w| {
                let utf16: Vec<u16> = l.name.encode_utf16().collect();
                w.be_u32(utf16.len() as u32);
                for u in utf16 {
                    w.be_u16(u);
                }
            });
        }
        // Section divider: what makes a group a group.
        if is_divider {
            additional(&mut extra, b"lsct", |w| {
                let kind = match entry {
                    Entry::GroupEnd(l) => {
                        if l.expanded {
                            1u32
                        } else {
                            2
                        }
                    }
                    _ => 3,
                };
                w.be_u32(kind);
            });
        }

        records.be_u32(extra.bytes.len() as u32);
        records.raw(&extra.bytes);

        for (counts, data) in packed_channels {
            // Written even when empty: the record declared two bytes for the
            // compression tag, and a reader counts on finding them.
            channel_data.be_u16(1); // RLE
            for c in counts {
                channel_data.be_u16(c);
            }
            channel_data.raw(&data);
        }
    }

    let mut layer_info = Writer::new();
    let body_len = records.bytes.len() + channel_data.bytes.len();
    // The layer info block's own length, padded to an even number of bytes.
    let pad = body_len % 2;
    layer_info.be_u32((body_len + pad) as u32);
    layer_info.raw(&records.bytes);
    layer_info.raw(&channel_data.bytes);
    if pad != 0 {
        layer_info.u8(0);
    }

    let mut mask_info = Writer::new();
    mask_info.raw(&layer_info.bytes);
    mask_info.be_u32(0); // global layer mask info

    w.be_u32(mask_info.bytes.len() as u32);
    w.raw(&mask_info.bytes);

    // --- the flattened composite ------------------------------------------
    // What every reader shows, including the ones that ignore layers.
    let (cw, ch) = (doc.width as usize, doc.height as usize);
    let mut planes = vec![vec![0u8; cw * ch]; 4];
    for y in 0..ch {
        for x in 0..cw {
            let p = composite.get(x as i32, y as i32);
            let i = y * cw + x;
            planes[0][i] = p.r;
            planes[1][i] = p.g;
            planes[2][i] = p.b;
            planes[3][i] = p.a;
        }
    }
    w.be_u16(1); // RLE
    let packed: Vec<(Vec<u16>, Vec<u8>)> =
        planes.iter().map(|p| rle_plane(p, cw, ch)).collect();
    // All the row counts first, for every channel, then all the data.
    for (counts, _) in &packed {
        for c in counts {
            w.be_u16(*c);
        }
    }
    for (_, data) in &packed {
        w.raw(data);
    }

    Ok(w.bytes)
}

/// A layer mask record, or an empty one when there is no mask.
fn write_layer_mask(w: &mut Writer, layer: Option<&Layer>) {
    let Some(mask) = layer.and_then(|l| l.mask.as_ref()) else {
        w.be_u32(0);
        return;
    };
    let (mw, mh) = (mask.data.width() as i32, mask.data.height() as i32);
    w.be_u32(20);
    w.be_i32(mask.offset.1);
    w.be_i32(mask.offset.0);
    w.be_i32(mask.offset.1 + mh);
    w.be_i32(mask.offset.0 + mw);
    w.u8(0); // default colour
    // Bit 1 is *disabled*, the opposite way round from ours.
    w.u8(if mask.enabled { 0 } else { 0x02 });
    w.be_u16(0); // padding
}

/// An additional-layer-information block, whose length is filled in after the
/// body has been written.
fn additional(w: &mut Writer, key: &[u8; 4], body: impl FnOnce(&mut Writer)) {
    w.raw(RESOURCE_TAG);
    w.raw(key);
    let at = w.bytes.len();
    w.be_u32(0);
    let start = w.bytes.len();
    body(w);
    let len = w.bytes.len() - start;
    w.patch_be_u32(at, len as u32);
    if len % 2 != 0 {
        w.u8(0);
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Everything one layer record says about itself, before its pixels are read.
struct Record {
    rect: IRect,
    channels: Vec<(i16, usize)>,
    blend: BlendMode,
    opacity: f32,
    clipping: bool,
    visible: bool,
    lock_transparency: bool,
    name: String,
    /// `lsct`: 1 or 2 open a group, 3 marks the bottom of one.
    section: Option<u32>,
    mask: Option<(IRect, bool)>,
}

/// Parse a PSD into a document.
pub fn read(bytes: &[u8]) -> Result<Document, IoError> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != SIGNATURE {
        return Err(IoError::Malformed("not a PSD file".into()));
    }
    let version = r.be_u16()?;
    if version != 1 {
        return Err(IoError::Unsupported(format!(
            "PSD version {version} (PSB and later are not read yet)"
        )));
    }
    r.skip(6)?;
    let channels = r.be_u16()?;
    let height = r.be_u32()?;
    let width = r.be_u32()?;
    let depth = r.be_u16()?;
    let mode = r.be_u16()?;

    if width == 0 || height == 0 {
        return Err(IoError::Malformed("the file declares an empty canvas".into()));
    }
    if width > crate::MAX_DIMENSION || height > crate::MAX_DIMENSION {
        return Err(IoError::TooLarge(width, height, crate::MAX_DIMENSION));
    }
    if depth != 8 {
        return Err(IoError::Unsupported(format!("{depth}-bit PSD (only 8-bit is read)")));
    }
    if mode != 3 {
        return Err(IoError::Unsupported(format!(
            "PSD colour mode {mode} (only RGB is read)"
        )));
    }

    // Colour mode data and image resources are skipped wholesale.
    let n = r.be_u32()? as usize;
    r.skip(n)?;
    let n = r.be_u32()? as usize;
    r.skip(n)?;

    let mask_len = r.be_u32()? as usize;
    let mask_end = r.position() + mask_len;
    if mask_len > r.remaining() {
        return Err(IoError::Malformed("the layer section runs past the end".into()));
    }

    let mut doc = Document::new("Untitled", width, height, Background::Transparent);
    doc.tree = Default::default();
    doc.active = None;

    let mut built = false;
    if mask_len > 0 {
        let layer_len = r.be_u32()? as usize;
        if layer_len > 0 {
            let end = r.position() + layer_len;
            read_layers(&mut r, &mut doc)?;
            r.seek(end.min(bytes.len()))?;
            built = !doc.tree.is_empty();
        }
    }
    r.seek(mask_end.min(bytes.len()))?;

    // A file with no layer section — or one this reader made nothing of — is
    // still openable: the composite every PSD carries becomes one layer.
    if !built {
        let composite = read_composite(&mut r, width, height, channels)?;
        let id = doc.tree.alloc_id();
        let mut layer = Layer::raster(id, "Background", composite);
        layer.is_background = true;
        doc.tree.push(layer, None);
    }

    doc.active = doc.tree.root().last().copied();
    doc.selected_layers = doc.active.into_iter().collect();
    doc.modified = false;
    Ok(doc)
}

fn read_layers(r: &mut Reader<'_>, doc: &mut Document) -> Result<(), IoError> {
    let count = r.be_i16()?;
    // Negative means the first alpha channel holds transparency; either way
    // the magnitude is the layer count.
    let count = count.unsigned_abs() as usize;
    if count > 8192 {
        return Err(IoError::Malformed(format!("the file claims {count} layers")));
    }

    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(read_record(r)?);
    }

    // Channel data follows every record, in the same order.
    let mut pixels: Vec<Option<PixelBuffer>> = Vec::with_capacity(count);
    let mut masks: Vec<Option<MaskBuffer>> = Vec::with_capacity(count);
    for rec in &records {
        let (px, mask) = read_channels(r, rec)?;
        pixels.push(px);
        masks.push(mask);
    }

    // Bottom to top, opening a group at its bounding divider and closing it at
    // the header that carries the name.
    let mut stack: Vec<Option<LayerId>> = vec![None];
    let mut pending: Vec<Vec<LayerId>> = vec![Vec::new()];
    for (i, rec) in records.into_iter().enumerate() {
        match rec.section {
            Some(3) => {
                // Bottom of a group: everything until its header belongs to it.
                stack.push(None);
                pending.push(Vec::new());
            }
            Some(1) | Some(2) => {
                let id = doc.tree.alloc_id();
                let mut group = Layer::group(id, rec.name.clone());
                group.expanded = rec.section == Some(1);
                apply_record(&mut group, &rec);
                let parent = *stack.get(stack.len().saturating_sub(2)).unwrap_or(&None);
                doc.tree.insert(group, parent, doc.tree.children(parent).len());
                // Re-home the children collected since the divider.
                if let Some(children) = pending.pop() {
                    for (n, child) in children.into_iter().enumerate() {
                        doc.tree.move_to(child, Some(id), n);
                    }
                }
                stack.pop();
                if let Some(top) = pending.last_mut() {
                    top.push(id);
                }
            }
            _ => {
                let Some(px) = pixels[i].clone() else { continue };
                let id = doc.tree.alloc_id();
                let mut layer = Layer::raster(id, rec.name.clone(), px);
                layer.offset = (rec.rect.x0, rec.rect.y0);
                apply_record(&mut layer, &rec);
                if let (Some(data), Some((mrect, enabled))) = (masks[i].clone(), rec.mask) {
                    layer.mask = Some(LayerMask {
                        data,
                        offset: (mrect.x0, mrect.y0),
                        enabled,
                        linked: true,
                    });
                }
                let parent = *stack.last().unwrap_or(&None);
                doc.tree.insert(layer, parent, doc.tree.children(parent).len());
                if let Some(top) = pending.last_mut() {
                    top.push(id);
                }
            }
        }
    }
    Ok(())
}

fn apply_record(layer: &mut Layer, rec: &Record) {
    layer.blend_mode = rec.blend;
    layer.opacity = rec.opacity;
    layer.clipping = rec.clipping;
    layer.visible = rec.visible;
    layer.locks.transparency = rec.lock_transparency;
}

fn read_record(r: &mut Reader<'_>) -> Result<Record, IoError> {
    let top = r.be_i32()?;
    let left = r.be_i32()?;
    let bottom = r.be_i32()?;
    let right = r.be_i32()?;
    let rect = IRect::new(left, top, right, bottom);

    let n = r.be_u16()? as usize;
    if n > 64 {
        return Err(IoError::Malformed(format!("a layer claims {n} channels")));
    }
    let mut channels = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.be_i16()?;
        let len = r.be_u32()? as usize;
        channels.push((id, len));
    }

    if r.take(4)? != RESOURCE_TAG {
        return Err(IoError::Malformed("a layer record is not tagged 8BIM".into()));
    }
    let blend = blend_from_key(r.take(4)?);
    let opacity = r.u8()? as f32 / 255.0;
    let clipping = r.u8()? != 0;
    let flags = r.u8()?;
    r.u8()?; // filler

    let extra_len = r.be_u32()? as usize;
    let extra_end = r.position() + extra_len;
    if extra_len > r.remaining() {
        return Err(IoError::Malformed("a layer's extra data runs past the end".into()));
    }

    // --- layer mask ---
    let mask_len = r.be_u32()? as usize;
    let mask_end = r.position() + mask_len;
    let mask = if mask_len >= 18 {
        let t = r.be_i32()?;
        let l = r.be_i32()?;
        let b = r.be_i32()?;
        let rr = r.be_i32()?;
        let _default = r.u8()?;
        let mflags = r.u8()?;
        Some((IRect::new(l, t, rr, b), mflags & 0x02 == 0))
    } else {
        None
    };
    r.seek(mask_end.min(extra_end))?;

    // --- blending ranges ---
    let n = r.be_u32()? as usize;
    r.skip(n.min(r.remaining()))?;

    // --- name, then the additional blocks ---
    let name_len = r.u8()? as usize;
    let raw = r.take(name_len)?.to_vec();
    let mut name = String::from_utf8_lossy(&raw).into_owned();
    // The Pascal string is padded so the whole field is a multiple of four.
    let pad = (4 - (1 + name_len) % 4) % 4;
    r.skip(pad.min(r.remaining()))?;

    let mut section = None;
    while r.position() + 12 <= extra_end {
        if r.take(4)? != RESOURCE_TAG {
            break;
        }
        let key: [u8; 4] = r.take(4)?.try_into().unwrap();
        let len = r.be_u32()? as usize;
        let body_end = r.position() + len;
        if len > r.remaining() {
            break;
        }
        match &key {
            b"lsct" => {
                if len >= 4 {
                    section = Some(r.be_u32()?);
                }
            }
            b"luni" => {
                // The unicode name is authoritative where both are present.
                let chars = r.be_u32()? as usize;
                if chars <= len {
                    let mut units = Vec::with_capacity(chars);
                    for _ in 0..chars {
                        units.push(r.be_u16()?);
                    }
                    if let Ok(s) = String::from_utf16(&units) {
                        if !s.is_empty() {
                            name = s;
                        }
                    }
                }
            }
            _ => {}
        }
        // Blocks are padded to an even length.
        r.seek((body_end + body_end % 2).min(extra_end))?;
    }
    r.seek(extra_end)?;

    Ok(Record {
        rect,
        channels,
        blend,
        opacity,
        clipping,
        // Bit 1 means hidden.
        visible: flags & 0x02 == 0,
        lock_transparency: flags & 0x01 != 0,
        name: name.trim_end_matches('\0').to_string(),
        section,
        mask,
    })
}

/// Read one plane, whichever compression it uses.
fn read_plane(
    r: &mut Reader<'_>,
    width: usize,
    height: usize,
    len: usize,
) -> Result<Vec<u8>, IoError> {
    let end = r.position() + len;
    if len > r.remaining() {
        return Err(IoError::Malformed("channel data runs past the end".into()));
    }
    let compression = r.be_u16()?;
    let out = match compression {
        0 => r.take(width * height)?.to_vec(),
        1 => {
            // Row byte counts first, then the packed rows.
            r.skip(height * 2)?;
            let mut out = Vec::with_capacity(width * height);
            for _ in 0..height {
                out.extend_from_slice(&unpack_bits(r, width)?);
            }
            out
        }
        other => {
            return Err(IoError::Unsupported(format!("PSD channel compression {other}")));
        }
    };
    r.seek(end)?;
    Ok(out)
}

fn read_channels(
    r: &mut Reader<'_>,
    rec: &Record,
) -> Result<(Option<PixelBuffer>, Option<MaskBuffer>), IoError> {
    let (w, h) = (rec.rect.width() as usize, rec.rect.height() as usize);
    let mut colour = [Vec::new(), Vec::new(), Vec::new()];
    let mut alpha: Option<Vec<u8>> = None;
    let mut mask_plane: Option<Vec<u8>> = None;

    for (id, len) in &rec.channels {
        match *id {
            -2 => {
                // The user mask has its own rect.
                let (mw, mh) = rec
                    .mask
                    .map(|(m, _)| (m.width() as usize, m.height() as usize))
                    .unwrap_or((0, 0));
                mask_plane = Some(read_plane(r, mw, mh, *len)?);
            }
            -1 => alpha = Some(read_plane(r, w, h, *len)?),
            0..=2 => colour[*id as usize] = read_plane(r, w, h, *len)?,
            // Anything else — a second user mask, say — is skipped.
            _ => {
                r.skip((*len).min(r.remaining()))?;
            }
        }
    }

    if w == 0 || h == 0 || colour[0].len() < w * h {
        return Ok((None, None));
    }
    let mut px = PixelBuffer::new(w as u32, h as u32);
    let get = |plane: &Vec<u8>, i: usize| plane.get(i).copied().unwrap_or(0);
    for i in 0..w * h {
        px.pixels_mut()[i] = Rgba8::new(
            get(&colour[0], i),
            get(&colour[1], i),
            get(&colour[2], i),
            // No alpha channel means the layer is opaque.
            alpha.as_ref().map(|a| get(a, i)).unwrap_or(255),
        );
    }

    let mask = match (mask_plane, rec.mask) {
        (Some(plane), Some((mrect, _))) if !mrect.is_empty() => {
            MaskBuffer::from_bytes(mrect.width(), mrect.height(), plane)
        }
        _ => None,
    };
    Ok((Some(px), mask))
}

/// The flattened image every PSD carries at the end.
fn read_composite(
    r: &mut Reader<'_>,
    width: u32,
    height: u32,
    channels: u16,
) -> Result<PixelBuffer, IoError> {
    let (w, h) = (width as usize, height as usize);
    let n = channels.clamp(1, 8) as usize;
    let compression = r.be_u16()?;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(n);

    match compression {
        0 => {
            for _ in 0..n {
                planes.push(r.take(w * h)?.to_vec());
            }
        }
        1 => {
            // Every channel's row counts come first, then all the data.
            r.skip(n * h * 2)?;
            for _ in 0..n {
                let mut plane = Vec::with_capacity(w * h);
                for _ in 0..h {
                    plane.extend_from_slice(&unpack_bits(r, w)?);
                }
                planes.push(plane);
            }
        }
        other => return Err(IoError::Unsupported(format!("PSD image compression {other}"))),
    }

    let mut px = PixelBuffer::new(width, height);
    let get = |c: usize, i: usize| planes.get(c).and_then(|p| p.get(i)).copied().unwrap_or(0);
    for i in 0..w * h {
        px.pixels_mut()[i] = Rgba8::new(
            get(0, i),
            get(1, i),
            get(2, i),
            if n >= 4 { get(3, i) } else { 255 },
        );
    }
    Ok(px)
}
