//! The healing brush, against the thing it exists to beat: the clone stamp.

use cshop_core::color::Rgba8;
use cshop_core::geom::IRect;
use cshop_core::heal::Heal;
use cshop_core::pixels::PixelBuffer;

/// A picture with a brightness gradient down it and fine texture on top — a
/// cheek, a wall, a sky. Cloning across a gradient is exactly what goes wrong.
fn graded(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let base = 60.0 + y as f32 / h as f32 * 140.0;
            // A deterministic speckle, so "texture" means something.
            let n = ((x * 7 + y * 13) % 11) as f32 - 5.0;
            let v = (base + n * 3.0).clamp(0.0, 255.0) as u8;
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    px
}

fn with_blemish(mut px: PixelBuffer, at: (i32, i32), r: i32) -> PixelBuffer {
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                px.set(at.0 + x, at.1 + y, Rgba8::opaque(20, 20, 20));
            }
        }
    }
    px
}

/// The measurement that matters: how far the repaired area sits from the
/// brightness its neighbours have. A clone brings the source's own tone with
/// it and lands wrong; healing keeps the destination's.
fn tone_error(px: &PixelBuffer, at: (i32, i32), r: i32) -> f32 {
    let mean = |cx: i32, cy: i32, rad: i32| {
        let (mut sum, mut n) = (0.0, 0.0);
        for y in -rad..=rad {
            for x in -rad..=rad {
                if x * x + y * y <= rad * rad {
                    sum += px.get(cx + x, cy + y).r as f32;
                    n += 1.0;
                }
            }
        }
        sum / n
    };
    // The patch against the ring of picture just outside it.
    let inner = mean(at.0, at.1, r - 1);
    let above = mean(at.0, at.1 - r * 2, r - 1);
    let below = mean(at.0, at.1 + r * 2, r - 1);
    (inner - (above + below) / 2.0).abs()
}

#[test]
fn healing_lands_on_the_tone_it_replaces_and_cloning_does_not() {
    let (spot, r) = ((64, 80), 8);
    let clean = graded(128, 160);
    let damaged = with_blemish(clean.clone(), spot, r);
    // A donor well up the gradient, which is where cloning goes wrong.
    let donor = (0, -40);

    // What the clone stamp would do: copy the pixels as they are.
    let mut cloned = damaged.clone();
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                let c = damaged.get(spot.0 + x + donor.0, spot.1 + y + donor.1);
                cloned.set(spot.0 + x, spot.1 + y, c);
            }
        }
    }

    // What healing does with the same donor.
    let mut heal = Heal::new(damaged.clone(), damaged.clone(), donor);
    let rect = IRect::new(spot.0 - r, spot.1 - r, spot.0 + r + 1, spot.1 + r + 1);
    heal.prepare(rect);
    let mut healed = damaged.clone();
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                healed.set(spot.0 + x, spot.1 + y, heal.at(spot.0 + x, spot.1 + y));
            }
        }
    }

    let (was, cloned_err, healed_err) = (
        tone_error(&damaged, spot, r),
        tone_error(&cloned, spot, r),
        tone_error(&healed, spot, r),
    );
    println!("blemish {was:.1}, cloned {cloned_err:.1}, healed {healed_err:.1}");
    assert!(cloned_err < was, "cloning is still better than the blemish");
    assert!(
        healed_err < cloned_err / 2.0,
        "healing should land far closer: cloned is off by {cloned_err:.1}, healed by {healed_err:.1}"
    );
    assert!(healed_err < 4.0, "and close in absolute terms: {healed_err:.1}");
}

/// Texture is the half healing takes from the source, so it has to arrive:
/// a repair that is smooth where its surroundings are speckled is a smudge,
/// not a repair.
#[test]
fn the_repair_keeps_the_grain() {
    let (spot, r) = ((64, 80), 8);
    let damaged = with_blemish(graded(128, 160), spot, r);
    let mut heal = Heal::new(damaged.clone(), damaged.clone(), (0, -40));
    let rect = IRect::new(spot.0 - r, spot.1 - r, spot.0 + r + 1, spot.1 + r + 1);
    heal.prepare(rect);

    let spread = |f: &dyn Fn(i32, i32) -> u8| {
        let v: Vec<f32> = (-4..=4)
            .flat_map(|y| (-4..=4).map(move |x| (x, y)))
            .map(|(x, y)| f(spot.0 + x, spot.1 + y) as f32)
            .collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|a| (a - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    };
    let grain_around = spread(&|x, y| damaged.get(x, y - 30).r);
    let grain_healed = spread(&|x, y| heal.at(x, y).r);
    assert!(
        grain_healed > grain_around * 0.5,
        "the repair should be about as speckled as its surroundings: {grain_healed:.2} against {grain_around:.2}"
    );
}

/// The spot form takes no source. It has to find one, and it must not find
/// the blemish itself or a place off the edge of the picture.
#[test]
fn the_spot_form_finds_its_own_donor() {
    let (spot, r) = ((64, 80), 8);
    let damaged = with_blemish(graded(128, 160), spot, r);
    let mut heal = Heal::spot(damaged.clone(), spot, r as f32);
    let off = heal.offset();
    assert!(off != (0, 0), "it has to look somewhere other than at itself");
    assert!(
        (off.0 * off.0 + off.1 * off.1) as f32 > (r as f32 * 1.4).powi(2),
        "and far enough not to be sampling the blemish: {off:?}"
    );

    let rect = IRect::new(spot.0 - r, spot.1 - r, spot.0 + r + 1, spot.1 + r + 1);
    heal.prepare(rect);
    let mut healed = damaged.clone();
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                healed.set(spot.0 + x, spot.1 + y, heal.at(spot.0 + x, spot.1 + y));
            }
        }
    }
    let err = tone_error(&healed, spot, r);
    assert!(err < 4.0, "and the repair still lands on the right tone: {err:.1}");
}

/// The blur is the expensive part, and doing it per dab is the whole reason
/// this is usable on a photograph. A dab must not cost more than the dab.
#[test]
fn a_dab_costs_the_dab_and_not_the_layer() {
    let big = graded(4000, 3000);
    let mut heal = Heal::new(big.clone(), big.clone(), (40, 0));
    let rect = IRect::new(2000, 1500, 2060, 1560); // a 60px dab
    let t = std::time::Instant::now();
    for _ in 0..20 {
        heal.prepare(rect);
    }
    let each = t.elapsed() / 20;
    println!("a 60px dab on a 12 MP layer: {each:?}");
    assert!(
        each < std::time::Duration::from_millis(10),
        "a dab took {each:?}; blurring the whole layer would have been ~250 ms, which is what this avoids"
    );
}
