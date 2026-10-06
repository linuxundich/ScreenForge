//! Perspective tilt for screenshots. Cairo only knows affine transforms, so
//! a tilted element is rendered flat into a bitmap first and then warped
//! onto its projected quadrilateral here, pixel by pixel: an inverse
//! homography maps every destination pixel back into the flat bitmap,
//! which is sampled bilinearly, and pixels near the quad's edges get
//! partial coverage so the outline stays smooth.

use cairo::{Format, ImageSurface};

use crate::render::RenderError;

pub type Quad = [(f64, f64); 4];

/// The four corners (top-left, top-right, bottom-right, bottom-left) of a
/// `w`×`h` rectangle turned by `tilt_x_deg` around its horizontal axis and
/// `tilt_y_deg` around its vertical axis, seen in perspective, scaled down
/// if needed to fit the original rectangle and centered in it.
pub fn tilted_quad(w: f64, h: f64, tilt_x_deg: f64, tilt_y_deg: f64) -> Quad {
    let (tx, ty) = (tilt_x_deg.clamp(-60.0, 60.0).to_radians(), tilt_y_deg.clamp(-60.0, 60.0).to_radians());
    let distance = 2.2 * w.max(h);
    let project = |x: f64, y: f64| {
        // Around the vertical axis: positive turns the right edge away.
        let x1 = x * ty.cos();
        let z1 = -x * ty.sin();
        // Around the horizontal axis: positive tips the top edge away.
        let y2 = y * tx.cos() - z1 * tx.sin();
        let z2 = y * tx.sin() + z1 * tx.cos();
        let f = distance / (distance - z2);
        (x1 * f, y2 * f)
    };
    let corners = [project(-w / 2.0, -h / 2.0), project(w / 2.0, -h / 2.0), project(w / 2.0, h / 2.0), project(-w / 2.0, h / 2.0)];
    let (min_x, max_x) = corners.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (min_y, max_y) = corners.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let k = (w / (max_x - min_x)).min(h / (max_y - min_y)).min(1.0);
    let (cx, cy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    corners.map(|(x, y)| (w / 2.0 + (x - cx) * k, h / 2.0 + (y - cy) * k))
}

/// 3×3 matrix mapping the unit square onto `quad` (Heckbert's square-to-
/// quad projective mapping).
fn square_to_quad(q: &Quad) -> [[f64; 3]; 3] {
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = *q;
    let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
    let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
    let (g, h) = if dx3.abs() < 1e-12 && dy3.abs() < 1e-12 {
        (0.0, 0.0)
    } else {
        let det = dx1 * dy2 - dx2 * dy1;
        ((dx3 * dy2 - dx2 * dy3) / det, (dx1 * dy3 - dx3 * dy1) / det)
    };
    [[x1 - x0 + g * x1, x3 - x0 + h * x3, x0], [y1 - y0 + g * y1, y3 - y0 + h * y3, y0], [g, h, 1.0]]
}

fn invert(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        [(e * i - f * h) * inv, (c * h - b * i) * inv, (b * f - c * e) * inv],
        [(f * g - d * i) * inv, (a * i - c * g) * inv, (c * d - a * f) * inv],
        [(d * h - e * g) * inv, (b * g - a * h) * inv, (a * e - b * d) * inv],
    ])
}

/// Warps `src` onto `quad` (in `src`'s own pixel space). Returns the warped
/// bitmap, covering the quad's bounding box, and that box's top-left.
pub fn warp(src: &mut ImageSurface, quad: &Quad) -> Result<(ImageSurface, f64, f64), RenderError> {
    let (sw, sh) = (src.width(), src.height());
    let min_x = quad.iter().map(|p| p.0).fold(f64::MAX, f64::min).floor();
    let min_y = quad.iter().map(|p| p.1).fold(f64::MAX, f64::min).floor();
    let max_x = quad.iter().map(|p| p.0).fold(f64::MIN, f64::max).ceil();
    let max_y = quad.iter().map(|p| p.1).fold(f64::MIN, f64::max).ceil();
    let (dw, dh) = (((max_x - min_x) as i32).max(1), ((max_y - min_y) as i32).max(1));
    let mut dest = ImageSurface::create(Format::ARgb32, dw, dh)?;
    let Some(inv) = invert(&square_to_quad(quad)) else { return Ok((dest, min_x, min_y)) };

    let src_stride = src.stride() as usize;
    let src_data = src.data()?;
    let dst_stride = dest.stride() as usize;
    {
        let mut dst = dest.data()?;
        let sample = |x: f64, y: f64| -> [f64; 4] {
            let x = x.clamp(0.0, (sw - 1) as f64);
            let y = y.clamp(0.0, (sh - 1) as f64);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(sw as usize - 1), (y0 + 1).min(sh as usize - 1));
            let (fx, fy) = (x - x0 as f64, y - y0 as f64);
            let px = |xx: usize, yy: usize, c: usize| src_data[yy * src_stride + xx * 4 + c] as f64;
            let mut out = [0.0; 4];
            for (c, o) in out.iter_mut().enumerate() {
                let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
                let bottom = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
                *o = top * (1.0 - fy) + bottom * fy;
            }
            out
        };
        for j in 0..dh {
            for i in 0..dw {
                let (x, y) = (min_x + i as f64 + 0.5, min_y + j as f64 + 0.5);
                let w = inv[2][0] * x + inv[2][1] * y + inv[2][2];
                if w.abs() < 1e-12 {
                    continue;
                }
                let u = (inv[0][0] * x + inv[0][1] * y + inv[0][2]) / w;
                let v = (inv[1][0] * x + inv[1][1] * y + inv[1][2]) / w;
                let (su, sv) = (u * sw as f64, v * sh as f64);
                // Coverage: 1 inside, fading over about one pixel outside.
                let coverage = (su + 0.5).min(sw as f64 - su + 0.5).min(sv + 0.5).min(sh as f64 - sv + 0.5).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let p = sample(su - 0.5, sv - 0.5);
                let o = j as usize * dst_stride + i as usize * 4;
                for c in 0..4 {
                    dst[o + c] = (p[c] * coverage).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    dest.mark_dirty();
    Ok((dest, min_x, min_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tilt_keeps_the_rectangle() {
        let q = tilted_quad(100.0, 200.0, 0.0, 0.0);
        let expected = [(0.0, 0.0), (100.0, 0.0), (100.0, 200.0), (0.0, 200.0)];
        for (a, b) in q.iter().zip(expected.iter()) {
            assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9, "{q:?}");
        }
    }

    #[test]
    fn turning_right_edge_away_makes_it_shorter_and_stays_inside() {
        let q = tilted_quad(100.0, 200.0, 0.0, 30.0);
        let left = q[3].1 - q[0].1;
        let right = q[2].1 - q[1].1;
        assert!(right < left, "right edge {right} should be shorter than left {left}");
        assert!(q.iter().all(|p| p.0 >= -1e-9 && p.0 <= 100.0 + 1e-9 && p.1 >= -1e-9 && p.1 <= 200.0 + 1e-9));
    }

    #[test]
    fn warping_maps_the_corners_onto_the_quad() {
        let mut src = ImageSurface::create(Format::ARgb32, 40, 80).unwrap();
        {
            let ctx = cairo::Context::new(&src).unwrap();
            ctx.set_source_rgb(1.0, 0.0, 0.0);
            ctx.paint().unwrap();
        }
        let quad = tilted_quad(40.0, 80.0, 10.0, 25.0);
        let (mut out, ox, oy) = warp(&mut src, &quad).unwrap();
        let stride = out.stride() as usize;
        let w = out.width() as usize;
        let data = out.data().unwrap();
        // The quad's center is opaque red; the box's top-right corner lies
        // outside the quad (top edge tipped away, right edge turned away).
        let cx = ((quad.iter().map(|p| p.0).sum::<f64>() / 4.0) - ox) as usize;
        let cy = ((quad.iter().map(|p| p.1).sum::<f64>() / 4.0) - oy) as usize;
        assert_eq!(data[cy * stride + cx * 4 + 3], 255);
        assert_eq!(data[(w - 1) * 4 + 3], 0);
    }
}
