//! # cshop-core
//!
//! The document model for C-Shop: layers, blend maths, pixel and mask storage,
//! and the undo system. Deliberately free of GPU, UI and file-format
//! dependencies so it stays fast to test and impossible to entangle.
//!
//! ## Conventions
//!
//! * **Stacking order** — index `0` is the *bottom* of a layer stack, matching
//!   document and PSD order. The Layers panel reverses this for display.
//! * **Alpha** — straight (non-premultiplied) everywhere in this crate.
//!   Premultiplication is an implementation detail of the GPU compositor.
//! * **Colour space** — pixels are sRGB-encoded, and blending happens in that
//!   encoded space, as established editors do. See [`color`] for the rationale.
//! * **Coordinates** — integer document pixels. `IRect` maxima are exclusive.

pub mod adjust;
pub mod blend;
pub mod color;
pub mod curve;
pub mod document;
pub mod fill;
pub mod effects;
pub mod filters;
pub mod font;
pub mod shape;
pub mod smart;
pub mod text;
pub mod geom;
pub mod guides;
pub mod heal;
pub mod history;
pub mod json;
pub mod layer;
pub mod lens;
pub mod mask;
pub mod paint;
pub mod path;
pub mod pixels;
pub mod profile;
pub mod relight;
pub mod resample;
pub mod retouch;
pub mod selection;
pub mod snapshot;
pub mod transform;
pub mod tree;
pub mod wand;

pub use adjust::{AdjustKind, Adjustment, LevelsChannel};
pub use blend::BlendMode;
pub use color::{Rgba, Rgba8};
pub use fill::{BucketOptions, Gradient, GradientKind};
pub use filters::{Filter, FilterContext};
pub use curve::Curve;
pub use document::{AlphaChannel, Background, Dirty, Document, DocumentId, EditTarget};
pub use geom::{IRect, Vec2};
pub use history::{Command, History};
pub use layer::{FillStyle, Layer, LayerId, LayerKind, LayerLocks, LayerMask};
pub use mask::MaskBuffer;
pub use paint::{Brush, PaintMode, Stroke, StrokeSource};
pub use pixels::PixelBuffer;
pub use resample::Resampling;
pub use selection::{Rectf, Selection, SelectionMode};
pub use transform::{Handle, Transform};
pub use tree::{LayerPos, LayerTree};
pub use wand::WandOptions;
