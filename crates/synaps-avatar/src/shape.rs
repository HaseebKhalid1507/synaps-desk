//! Device-pixel shape helpers: scanline-fill polygons into X shape rectangles, so the
//! bounding shape (no compositor) follows rounded corners, the bubble tail and hearts.

pub type ShapeRect = (i16, i16, u16, u16);

/// Scanline fill (even-odd) of a polygon in device px.
pub fn poly_rows(pts: &[(f32, f32)], out: &mut Vec<ShapeRect>) {
    if pts.len() < 3 {
        return;
    }
    let ymin = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor() as i32;
    let ymax = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32;
    let mut xs: Vec<f32> = Vec::new();
    for y in ymin..ymax {
        let yc = y as f32 + 0.5;
        xs.clear();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if (a.1 <= yc && yc < b.1) || (b.1 <= yc && yc < a.1) {
                xs.push(a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            let (x0, x1) = (pair[0].round() as i32, pair[1].round() as i32);
            if x1 > x0 {
                out.push((x0 as i16, y as i16, (x1 - x0) as u16, 1));
            }
        }
    }
}

/// Classic parametric heart, `size` px wide, centred on (cx, cy).
pub fn heart(cx: f32, cy: f32, size: f32) -> Vec<(f32, f32)> {
    let k = size / 32.0;
    (0..40)
        .map(|i| {
            let t = i as f32 / 40.0 * std::f32::consts::TAU;
            let x = 16.0 * t.sin().powi(3);
            let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
            (cx + x * k, cy - y * k - 1.5 * k)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_fills() {
        let mut out = Vec::new();
        poly_rows(&[(0.0, 0.0), (10.0, 0.0), (10.0, 4.0), (0.0, 4.0)], &mut out);
        assert_eq!(out, vec![(0, 0, 10, 1), (0, 1, 10, 1), (0, 2, 10, 1), (0, 3, 10, 1)]);
    }
}
