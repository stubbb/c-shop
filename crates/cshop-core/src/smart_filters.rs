//! Filters attached to a layer instead of burned into it.
//!
//! # Why this and not just running the filter
//!
//! `Filter ▸ Gaussian Blur` replaces a layer's pixels with blurred ones. The
//! settings that produced them are gone the moment the dialog closes, so
//! "slightly less blur" means undo and start again, and "how much blur was
//! that?" has no answer at all. Adjustments escaped this years ago by becoming
//! layers; filters never did.
//!
//! A filter attached to a layer keeps its settings. The layer's own pixels are
//! never touched: the stack is evaluated on the way to the screen, so changing
//! a radius is changing a number, and removing a filter leaves no trace of it.
//!
//! # What a stack is
//!
//! An ordered list, applied bottom-first, each with its own switch and its own
//! opacity. Opacity blends the filtered result back over what went into it,
//! which is what makes "half a sharpen" possible without a second layer.
//!
//! A shared mask limits where the whole stack applies. One mask rather than
//! one per filter, because the question people actually ask is "sharpen the
//! eyes and not the skin", and asking it once is enough.

use crate::filters::{Filter, FilterContext};
use crate::mask::MaskBuffer;
use crate::pixels::PixelBuffer;

/// One filter in a stack.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSlot {
    pub filter: Filter,
    /// Off keeps the settings and stops applying them, which is how you find
    /// out what a filter was doing.
    pub enabled: bool,
    /// How much of the filtered result to keep, `0..=1`.
    pub opacity: f32,
}

impl FilterSlot {
    pub fn new(filter: Filter) -> FilterSlot {
        FilterSlot { filter, enabled: true, opacity: 1.0 }
    }
}

/// A layer's filter stack.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SmartFilters {
    /// The whole stack's switch, so all of it can be compared against none of
    /// it in one click.
    pub enabled: bool,
    pub slots: Vec<FilterSlot>,
    /// Where the stack applies. White applies it, black leaves the layer as it
    /// was, and the buffer is the layer's own size.
    pub mask: Option<MaskBuffer>,
}

impl SmartFilters {
    /// Whether this would change anything.
    pub fn any(&self) -> bool {
        self.enabled && self.slots.iter().any(|s| s.enabled)
    }

    /// How many are switched on, for the panel's summary.
    pub fn active(&self) -> usize {
        if !self.enabled {
            return 0;
        }
        self.slots.iter().filter(|s| s.enabled).count()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.slots.iter().map(|s| s.filter.name()).collect()
    }

    /// Run the stack over `src`.
    ///
    /// Returns `None` when there is nothing switched on, so a caller can use
    /// the layer's own pixels without copying them.
    pub fn render(&self, src: &PixelBuffer, ctx: &FilterContext) -> Option<PixelBuffer> {
        if !self.any() {
            return None;
        }
        let mut out = src.clone();
        for slot in self.slots.iter().filter(|s| s.enabled) {
            let filtered = slot.filter.apply(&out, ctx);
            let k = slot.opacity.clamp(0.0, 1.0);
            out = if k >= 1.0 { filtered } else { mix(&out, &filtered, k) };
        }
        // The mask is applied once, against the layer as it arrived, rather
        // than between filters: a stack is one effect as far as anyone using
        // it is concerned, and masking between steps would make the order of
        // the stack change what the mask meant.
        if let Some(mask) = &self.mask {
            out = through(src, &out, mask);
        }
        Some(out)
    }
}

/// `a` toward `b` by `k`, in premultiplied colour so a filter that changes
/// alpha does not drag colour out of transparent pixels.
fn mix(a: &PixelBuffer, b: &PixelBuffer, k: f32) -> PixelBuffer {
    let mut out = a.clone();
    for y in 0..a.height() as i32 {
        for x in 0..a.width() as i32 {
            out.set(x, y, blend(a.get(x, y), b.get(x, y), k));
        }
    }
    out
}

/// `before` where the mask is black, `after` where it is white.
fn through(before: &PixelBuffer, after: &PixelBuffer, mask: &MaskBuffer) -> PixelBuffer {
    let mut out = after.clone();
    for y in 0..before.height() as i32 {
        for x in 0..before.width() as i32 {
            let k = mask.get(x, y) as f32 / 255.0;
            out.set(x, y, blend(before.get(x, y), after.get(x, y), k));
        }
    }
    out
}

#[inline]
fn blend(a: crate::color::Rgba8, b: crate::color::Rgba8, k: f32) -> crate::color::Rgba8 {
    if k <= 0.0 {
        return a;
    }
    if k >= 1.0 {
        return b;
    }
    let (a, b) = (a.to_f32(), b.to_f32());
    let pm = |c: crate::color::Rgba| (c.r * c.a, c.g * c.a, c.b * c.a, c.a);
    let (ar, ag, ab, aa) = pm(a);
    let (br, bg, bb, ba) = pm(b);
    let (r, g, bl, al) = (
        ar + (br - ar) * k,
        ag + (bg - ag) * k,
        ab + (bb - ab) * k,
        aa + (ba - aa) * k,
    );
    if al <= 1e-6 {
        return crate::color::Rgba8::TRANSPARENT;
    }
    crate::color::Rgba { r: r / al, g: g / al, b: bl / al, a: al }.to_u8()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    fn edge(w: u32, h: u32) -> PixelBuffer {
        let mut px = PixelBuffer::new(w, h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = if x < w as i32 / 2 { 30 } else { 220 };
                px.set(x, y, Rgba8::opaque(v, v, v));
            }
        }
        px
    }

    fn slope(px: &PixelBuffer) -> i32 {
        let m = px.width() as i32 / 2;
        px.get(m + 1, 8).r as i32 - px.get(m - 2, 8).r as i32
    }

    #[test]
    fn an_empty_stack_costs_nothing_and_returns_nothing() {
        let px = edge(32, 16);
        let f = SmartFilters::default();
        assert!(f.render(&px, &FilterContext::default()).is_none());
        // Nor does one that is switched off.
        let off = SmartFilters {
            enabled: false,
            slots: vec![FilterSlot::new(Filter::GaussianBlur { radius: 4.0 })],
            mask: None,
        };
        assert!(off.render(&px, &FilterContext::default()).is_none());
    }

    #[test]
    fn the_layer_itself_is_never_touched() {
        let px = edge(32, 16);
        let before = px.clone();
        let f = SmartFilters {
            enabled: true,
            slots: vec![FilterSlot::new(Filter::GaussianBlur { radius: 3.0 })],
            mask: None,
        };
        let out = f.render(&px, &FilterContext::default()).unwrap();
        assert_eq!(px.pixels(), before.pixels(), "the source is read, not written");
        assert!(slope(&out) < slope(&before), "and the result is blurred");
    }

    /// Changing a radius is changing a number, so two stacks that differ only
    /// in the number give different answers from the same untouched layer —
    /// which is the whole difference from running the filter destructively.
    #[test]
    fn changing_a_setting_re_renders_from_the_original() {
        let px = edge(64, 16);
        let stack = |r: f32| SmartFilters {
            enabled: true,
            slots: vec![FilterSlot::new(Filter::GaussianBlur { radius: r })],
            mask: None,
        };
        let gentle = stack(2.0).render(&px, &FilterContext::default()).unwrap();
        let heavy = stack(8.0).render(&px, &FilterContext::default()).unwrap();
        assert!(
            slope(&heavy) < slope(&gentle),
            "more radius should be softer: {} against {}",
            slope(&heavy),
            slope(&gentle)
        );
        // And going back to the gentle one gives exactly the gentle one, not
        // a gentle blur of a heavy one.
        let again = stack(2.0).render(&px, &FilterContext::default()).unwrap();
        assert_eq!(again.pixels(), gentle.pixels());
    }

    #[test]
    fn opacity_blends_the_filter_back_over_what_went_in() {
        let px = edge(64, 16);
        let at = |k: f32| {
            SmartFilters {
                enabled: true,
                slots: vec![FilterSlot {
                    filter: Filter::GaussianBlur { radius: 6.0 },
                    enabled: true,
                    opacity: k,
                }],
                mask: None,
            }
            .render(&px, &FilterContext::default())
            .unwrap()
        };
        let (none, half, all) = (slope(&at(0.0)), slope(&at(0.5)), slope(&at(1.0)));
        assert_eq!(none, slope(&px), "at zero it is the layer");
        assert!(all < half && half < none, "and it arrives gradually: {none}, {half}, {all}");
    }

    #[test]
    fn a_stack_applies_in_order_and_a_switch_takes_one_out() {
        let px = edge(64, 16);
        let both = SmartFilters {
            enabled: true,
            slots: vec![
                FilterSlot::new(Filter::GaussianBlur { radius: 5.0 }),
                FilterSlot::new(Filter::UnsharpMask { radius: 3.0, amount: 1.5, threshold: 0.0 }),
            ],
            mask: None,
        };
        let blur_only = SmartFilters { slots: both.slots[..1].to_vec(), ..both.clone() };
        let ctx = FilterContext::default();
        let a = both.render(&px, &ctx).unwrap();
        let b = blur_only.render(&px, &ctx).unwrap();
        assert!(slope(&a) > slope(&b), "sharpening after blurring should recover some edge");

        // Switching the second one off gives the first one's answer exactly.
        let mut switched = both.clone();
        switched.slots[1].enabled = false;
        assert_eq!(switched.render(&px, &ctx).unwrap().pixels(), b.pixels());
        assert_eq!(switched.active(), 1);
    }

    #[test]
    fn a_mask_decides_where_the_stack_lands() {
        let px = edge(64, 16);
        let mut mask = MaskBuffer::hide_all(64, 16);
        // Reveal the right half only.
        for y in 0..16 {
            for x in 32..64 {
                mask.set(x, y, 255);
            }
        }
        let f = SmartFilters {
            enabled: true,
            slots: vec![FilterSlot::new(Filter::Solarize)],
            mask: Some(mask),
        };
        let out = f.render(&px, &FilterContext::default()).unwrap();
        let straight = SmartFilters { mask: None, ..f.clone() }
            .render(&px, &FilterContext::default())
            .unwrap();
        assert_eq!(out.get(10, 8), px.get(10, 8), "hidden: the layer as it was");
        assert_ne!(out.get(50, 8), px.get(50, 8), "revealed: the filter");
        assert_eq!(out.get(50, 8), straight.get(50, 8), "and the whole of it, not a part");
    }
}
