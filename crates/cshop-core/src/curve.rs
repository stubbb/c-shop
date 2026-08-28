//! Editable tone curves.
//!
//! Used by the Curves adjustment and by the gradient editor. Interpolation is
//! monotone cubic (Fritsch–Carlson): an ordinary Catmull–Rom spline overshoots
//! between close control points, which on a tone curve shows up as bright
//! pixels going *darker* as you drag a point up — visible, wrong, and a classic
//! bug in home-grown curve editors.

/// A curve through control points, evaluated over `0..=1` in both axes.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    /// Control points sorted by `x`, always containing at least two.
    points: Vec<(f32, f32)>,
}

impl Default for Curve {
    /// The identity curve: output equals input.
    fn default() -> Self {
        Self { points: vec![(0.0, 0.0), (1.0, 1.0)] }
    }
}

impl Curve {
    pub fn new(points: Vec<(f32, f32)>) -> Self {
        let mut c = Self { points };
        c.normalise();
        c
    }

    pub fn points(&self) -> &[(f32, f32)] {
        &self.points
    }

    pub fn is_identity(&self) -> bool {
        self.points.len() == 2
            && (self.points[0].0 - 0.0).abs() < 1e-6
            && (self.points[0].1 - 0.0).abs() < 1e-6
            && (self.points[1].0 - 1.0).abs() < 1e-6
            && (self.points[1].1 - 1.0).abs() < 1e-6
    }

    /// Add a control point, returning its index.
    pub fn add(&mut self, x: f32, y: f32) -> usize {
        self.points.push((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        self.normalise();
        self.points
            .iter()
            .position(|p| (p.0 - x.clamp(0.0, 1.0)).abs() < 1e-6)
            .unwrap_or(0)
    }

    /// Move a control point. The two endpoints keep their `x`, so the curve
    /// always spans the full range.
    pub fn move_point(&mut self, index: usize, x: f32, y: f32) {
        let Some(point) = self.points.get_mut(index) else { return };
        point.0 = x.clamp(0.0, 1.0);
        point.1 = y.clamp(0.0, 1.0);
        // normalise() re-pins the endpoints' x, so dragging one sideways only
        // moves it vertically rather than shortening the curve's range.
        self.normalise();
    }

    /// Remove a control point. The endpoints cannot be removed.
    pub fn remove(&mut self, index: usize) {
        if self.points.len() <= 2 || index == 0 || index + 1 == self.points.len() {
            return;
        }
        self.points.remove(index);
    }

    /// Index of the control point within `radius` of `(x, y)`, if any.
    pub fn hit(&self, x: f32, y: f32, radius: f32) -> Option<usize> {
        self.points
            .iter()
            .enumerate()
            .map(|(i, p)| (i, ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()))
            .filter(|(_, d)| *d <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Sort by `x`, drop duplicates, and pin the range to `0..=1`.
    fn normalise(&mut self) {
        self.points.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.points.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-4);
        if self.points.len() < 2 {
            self.points = vec![(0.0, 0.0), (1.0, 1.0)];
            return;
        }
        // Endpoints anchor the ends of the range so evaluation never
        // extrapolates.
        self.points.first_mut().expect("checked above").0 = 0.0;
        self.points.last_mut().expect("checked above").0 = 1.0;
    }

    /// Evaluate the curve at `x`, clamped to `0..=1`.
    pub fn eval(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let p = &self.points;
        if p.len() == 2 {
            // A straight line, which is the common case and worth not splining.
            let t = (x - p[0].0) / (p[1].0 - p[0].0).max(1e-6);
            return (p[0].1 + t * (p[1].1 - p[0].1)).clamp(0.0, 1.0);
        }

        // Locate the span containing x.
        let i = match p.binary_search_by(|q| q.0.total_cmp(&x)) {
            Ok(i) => return p[i].1,
            Err(0) => 0,
            Err(i) if i >= p.len() => p.len() - 2,
            Err(i) => i - 1,
        };

        let (x0, y0) = p[i];
        let (x1, y1) = p[i + 1];
        let h = (x1 - x0).max(1e-6);
        let t = (x - x0) / h;

        let (m0, m1) = self.tangents(i);
        // Cubic Hermite basis.
        let t2 = t * t;
        let t3 = t2 * t;
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * m0
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * h * m1;
        y.clamp(0.0, 1.0)
    }

    /// Fritsch–Carlson tangents for the span starting at `i`, limited so the
    /// interpolant cannot overshoot the control points.
    fn tangents(&self, i: usize) -> (f32, f32) {
        let p = &self.points;
        let slope = |a: usize, b: usize| (p[b].1 - p[a].1) / (p[b].0 - p[a].0).max(1e-6);

        let d = slope(i, i + 1);
        let m0 = if i == 0 { d } else { (slope(i - 1, i) + d) * 0.5 };
        let m1 = if i + 2 >= p.len() { d } else { (d + slope(i + 1, i + 2)) * 0.5 };

        // A flat span must stay flat, or the curve dips between equal points.
        if d.abs() < 1e-9 {
            return (0.0, 0.0);
        }
        // Clamp the tangents into the monotone region.
        let limit = |m: f32| {
            let a = m / d;
            match a {
                a if a < 0.0 => 0.0,
                a if a > 3.0 => 3.0 * d,
                _ => m,
            }
        };
        (limit(m0), limit(m1))
    }

    /// Sample the curve into a 256-entry table for the GPU and for fast CPU
    /// application.
    pub fn to_lut(&self) -> [u8; 256] {
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            *slot = (self.eval(i as f32 / 255.0) * 255.0 + 0.5) as u8;
        }
        lut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_curve_is_the_identity() {
        let c = Curve::default();
        assert!(c.is_identity());
        for i in 0..=10 {
            let x = i as f32 / 10.0;
            assert!((c.eval(x) - x).abs() < 1e-5, "identity failed at {x}");
        }
    }

    #[test]
    fn evaluation_is_clamped_to_the_unit_range() {
        let c = Curve::default();
        assert_eq!(c.eval(-5.0), 0.0);
        assert_eq!(c.eval(5.0), 1.0);
    }

    #[test]
    fn a_control_point_pulls_the_curve_through_itself() {
        let c = Curve::new(vec![(0.0, 0.0), (0.5, 0.8), (1.0, 1.0)]);
        assert!((c.eval(0.5) - 0.8).abs() < 1e-4);
        assert!((c.eval(0.0) - 0.0).abs() < 1e-5);
        assert!((c.eval(1.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_curve_is_monotone_and_never_overshoots() {
        // The bug this guards: a spline through closely spaced points dipping
        // below its own control points, so raising a highlight darkens it.
        let c = Curve::new(vec![(0.0, 0.0), (0.45, 0.9), (0.55, 0.92), (1.0, 1.0)]);
        let mut previous = -1.0;
        for i in 0..=500 {
            let x = i as f32 / 500.0;
            let y = c.eval(x);
            assert!(y >= previous - 1e-4, "curve went backwards at x={x}: {y} < {previous}");
            assert!((0.0..=1.0).contains(&y));
            previous = y;
        }
        // Between the two close points the curve must stay inside their values.
        let mid = c.eval(0.5);
        assert!((0.9 - 1e-3..=0.92 + 1e-3).contains(&mid), "overshot to {mid}");
    }

    #[test]
    fn a_flat_span_stays_flat() {
        let c = Curve::new(vec![(0.0, 0.5), (0.4, 0.5), (0.6, 0.5), (1.0, 1.0)]);
        for i in 0..=20 {
            let x = 0.4 + (i as f32 / 20.0) * 0.2;
            assert!((c.eval(x) - 0.5).abs() < 1e-3, "dipped at {x}: {}", c.eval(x));
        }
    }

    #[test]
    fn points_are_kept_sorted_and_deduplicated() {
        let c = Curve::new(vec![(1.0, 1.0), (0.3, 0.4), (0.0, 0.0), (0.3, 0.9)]);
        let xs: Vec<f32> = c.points().iter().map(|p| p.0).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "not sorted: {xs:?}");
        assert_eq!(c.points().len(), 3, "the duplicate x should have collapsed");
    }

    #[test]
    fn the_endpoints_cannot_be_removed_or_moved_off_the_ends() {
        let mut c = Curve::new(vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
        c.remove(0);
        c.remove(2);
        assert_eq!(c.points().len(), 3, "endpoints must survive");

        c.remove(1);
        assert_eq!(c.points().len(), 2);

        c.move_point(0, 0.7, 0.2);
        assert_eq!(c.points().first().unwrap().0, 0.0, "the first point stays at x=0");
        assert_eq!(c.points().last().unwrap().0, 1.0);
    }

    #[test]
    fn hit_testing_finds_the_nearest_point() {
        let c = Curve::new(vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
        assert_eq!(c.hit(0.52, 0.48, 0.06), Some(1));
        assert_eq!(c.hit(0.5, 0.5, 0.01), Some(1));
        assert_eq!(c.hit(0.25, 0.75, 0.05), None);
    }

    #[test]
    fn the_lut_matches_direct_evaluation() {
        let c = Curve::new(vec![(0.0, 0.1), (0.5, 0.7), (1.0, 0.95)]);
        let lut = c.to_lut();
        for i in [0usize, 1, 64, 128, 200, 255] {
            let direct = (c.eval(i as f32 / 255.0) * 255.0 + 0.5) as u8;
            assert_eq!(lut[i], direct, "LUT and eval disagree at {i}");
        }
    }

    #[test]
    fn an_inverting_curve_works() {
        let c = Curve::new(vec![(0.0, 1.0), (1.0, 0.0)]);
        assert!((c.eval(0.0) - 1.0).abs() < 1e-5);
        assert!((c.eval(1.0) - 0.0).abs() < 1e-5);
        assert!((c.eval(0.25) - 0.75).abs() < 1e-4);
    }
}
