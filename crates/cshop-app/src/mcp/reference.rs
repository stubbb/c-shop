//! The command reference, for a caller that arrives with no documentation.
//!
//! An agent reaching this server over a socket has the tool descriptions and
//! nothing else. Rather than expect it to have read the manual, `describe`
//! hands it the same reference a person would read.
//!
//! The lists that can go stale are derived from the code — blend modes come
//! from the enum, fonts from the machine's own scan — so this cannot quietly
//! describe an editor that no longer exists. The prose is written out, because
//! prose has nowhere else to live.

pub fn describe(topic: &str) -> String {
    match topic.trim().to_ascii_lowercase().as_str() {
        "" | "summary" | "overview" => summary(),
        "commands" | "command" => COMMANDS.to_string(),
        "syntax" => SYNTAX.to_string(),
        "filters" | "filter" => FILTERS.to_string(),
        "adjustments" | "adjustment" | "adjust" => ADJUSTMENTS.to_string(),
        "effects" | "effect" | "fx" => EFFECTS.to_string(),
        "blends" | "blend" | "blending" => blends(),
        "fonts" | "font" | "families" => fonts(),
        other => format!(
            "no topic called {other:?}. There are: commands, syntax, filters, \
             adjustments, effects, blends, fonts."
        ),
    }
}

fn summary() -> String {
    format!(
        "C-Shop drives from a script: one command per line, `#` starts a comment.\n\n\
         {SYNTAX}\n{COMMANDS}\n\
         Ask `describe` for filters, adjustments, effects, blends or fonts to see \
         what each takes.\n\n\
         A worked example:\n\n\
         {EXAMPLE}"
    )
}

const EXAMPLE: &str = "\
    open garden.jpg
    resize fit=1600
    style watercolour
    text 120 1400 \"Garden Walk\" family=\"C059\" size=110 color=#f5ecd8
    effect drop-shadow distance=6 size=10 opacity=0.5
    layer flatten
    export garden-final.jpg quality=92

Measure before placing: `measure text \"...\" size=110 family=\"C059\"` reports
the size it would draw without drawing it, which is how type gets centred
without guessing.";

const SYNTAX: &str = "\
SYNTAX
  One command per line. `#` starts a comment. Arguments are positional,
  options are key=value, strings are double-quoted with \\n, \\t and \\\" escapes.
  A bare word like `bold` means bold=true, so bare flags go last.

  Colours are #rgb, #rrggbb, #rrggbbaa, or one of: black white red green blue
  yellow orange purple grey transparent.

  Paths are relative to the workspace and cannot leave it.
";

const COMMANDS: &str = "\
COMMANDS
  new W H [background=]        start a document (white, transparent, or a colour)
  open PATH                    open an image, .psd or .cshop, with its layers
  place [PATH] [x= y=]         bring an image in as a layer above the active one;
                               with no path, re-places the file this was opened from
  resize W [H]                 resample; fit= scales the longest side, scale=
                               multiplies, one dimension derives the other;
                               filter=nearest|bilinear|bicubic|lanczos, or `canvas`
                               to pad and crop instead of scaling
  text X Y \"...\"               a type layer, baseline starting at X Y
                               size= color= family= bold italic align= leading=
                               tracking= wrap=
  measure text \"...\"           report the size those options would draw
  shape KIND X Y W H           rect ellipse polygon star line
                               fill= stroke= stroke-width= stroke-align= radius=
                               sides= inner= thickness=
  fill COLOUR                  fill the layer, or the selection if there is one
  gradient X1 Y1 X2 Y2         from= to= style= blend= opacity= reverse
                               colours carry alpha, so from=#00000000 is a wash
  select X Y W H | all | none  feather= softens the edge
  filter NAME                  see `describe filters`
  adjust NAME                  see `describe adjustments`; `as-layer` keeps it editable
  effect NAME                  see `describe effects`; repeat to stack
  style NAME [param=value]     apply a style; see `list_styles`
  layer WHAT                   new group duplicate delete merge-down flatten
                               rasterize select <index>
  set key=value                opacity= fill-opacity= name= blend= on the active layer
  move DX DY                   nudge the active layer
  order WHERE                  top bottom up down
  info                         report size, layer count, bit depth and profile
  mode [8|16]                  bits a channel; widening is lossless, narrowing is not
  export PATH [quality=]       write it; .cshop and .psd keep layers, the rest flatten
                               depth=16 writes sixteen bits (PNG and TIFF only)
";

const FILTERS: &str = "\
FILTERS — `filter NAME [options]`
  gaussian-blur radius=      the ordinary blur
  box-blur radius=
  motion-blur radius= angle=
  surface-blur radius= threshold=
      Edge-preserving. The threshold is the tolerance within which pixels may
      average, so a small one flattens the inside of a shape while refusing to
      average across its edge. Open it up and the radius stops being reach and
      starts being smear.
  sharpen amount=
  unsharp-mask radius= amount= threshold=
  add-noise amount= [monochromatic]
      Monochrome noise reads as film; coloured noise reads as a broken sensor.
  high-pass radius=
  find-edges                  dark lines on white, which is what an ink layer wants
  median radius=              moves each pixel to a value that occurs nearby, so
                              areas collapse into flat patches with clean borders
  mosaic size=
  crystallize size= seed=
  emboss angle= height= amount=
  solarize
  diffuse amount= seed=
  twirl angle=
";

const ADJUSTMENTS: &str = "\
ADJUSTMENTS — `adjust NAME [options]`, or add `as-layer` to keep it editable
  brightness-contrast brightness= contrast=
  levels black= white= gamma= out-black= out-white=
      Where the ends of the tonal range sit. out-black=0.16 means no pixel may
      be darker than that, which is how faded film is made — no reduction in
      contrast alone reproduces it.
  gradient-map from= to= [mid= midpoint=]
      Replaces colour with a ramp indexed by brightness. Two inks, a blueprint.
  photo-filter color= density= [preserve-luminosity=]
      A cast over everything, without the tonal shift an overlay brings.
  hue-saturation hue= saturation= lightness=
  vibrance vibrance=
  exposure exposure= offset= gamma=
  invert
  posterize levels=
      Quantises each channel independently, so flatten the picture first or
      neighbouring pixels land on opposite sides of a band and the result is
      speckle rather than flat colour.
  threshold level=
  black-and-white [tint=]
";

const EFFECTS: &str = "\
EFFECTS — `effect NAME [options]` on the active layer; repeat to stack, `effect none` clears
  drop-shadow    color= opacity= size= distance= angle= spread= blend=
  inner-shadow   the same options
  outer-glow     color= opacity= size= spread= blend=
  inner-glow     the same options
  bevel          size= depth= soften= angle= altitude= style=inner|outer|emboss|pillow
  emboss         bevel, with the emboss style already chosen
  satin          color= opacity= size= distance= angle= blend=
  color-overlay  color= opacity= blend=
  gradient-overlay from= to= opacity= angle= scale= style= reverse blend=
  pattern-overlay  pattern= color= background= scale= angle= opacity= blend=
  stroke         size= color= position=inside|center|outside opacity= blend=

Effects are drawn outside the layer's fill opacity, so `set fill-opacity=0.2`
fades the pixels while leaving a stroke or a pattern at full strength.
";

fn blends() -> String {
    let names: Vec<&str> = cshop_core::blend::BlendMode::all().map(|m| m.name()).collect();
    format!(
        "BLEND MODES — `set blend=\"NAME\"`, or blend= on a gradient or an effect\n\n  {}\n\n\
         Blending is done in the gamma-encoded space these values are stored in, \
         which is what established editors do and what makes the results match \
         what anyone would expect from them.\n\n\
         Two worth knowing: Color takes hue and saturation from the top layer and \
         lightness from underneath, so it cannot put colour where the backdrop has \
         no midtones. Luminosity is its dual — the same result at full opacity, \
         differing only in what partial opacity fades toward.",
        names.join("  ")
    )
}

fn fonts() -> String {
    let db = cshop_core::font::FontDb::global();
    let mut names: Vec<&str> = db.families().iter().map(|f| f.name.as_str()).collect();
    names.sort_unstable();
    format!(
        "FONT FAMILIES — `text ... family=\"NAME\"`\n\n\
         {} installed on this machine:\n\n  {}\n\n\
         `measure text \"...\" family=\"NAME\" size=N` reports what a family would \
         draw before anything is committed, which is how to find out whether one \
         is really there.",
        names.len(),
        names.join("\n  ")
    )
}
