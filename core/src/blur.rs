//! A box blur approximating a Gaussian blur, for the shadow blur radius
//! (`ShadowParams::blur`) — Cairo has no native blur filter, so this runs
//! directly on a premultiplied-ARGB32 pixel buffer. Three passes of a
//! sliding-window box blur (horizontal then vertical) is the standard
//! cheap approximation and is what most 2D engines without a real Gaussian
//! filter use.
//!
//! Deliberately independent of `cairo` — it operates on a plain byte
//! buffer plus width/height/stride, so it's testable without constructing
//! a surface. `crate::render` is the only caller, and it's the one that
//! extracts/reinserts a `cairo::ImageSurface`'s pixel data around this.

const PASSES: usize = 3;

/// Blurs `data` in place — a premultiplied-ARGB32 (4 bytes/pixel) buffer,
/// `width`×`height` pixels, `stride` bytes per row (may exceed
/// `width * 4` if the surface pads rows). `radius` in pixels; `<= 0` is a
/// no-op. Out-of-bounds reads at the buffer's edges are clamped (i.e. the
/// edge pixel is treated as extending outward) rather than treated as
/// zero, so a shape that already touches the buffer's edge won't darken
/// there — callers that want the blur to fully dissipate should pad the
/// buffer well beyond the shape's own bounds first.
pub fn box_blur(data: &mut [u8], width: i32, height: i32, stride: i32, radius: f64) {
    let radius = radius.round() as i32;
    if radius <= 0 || width <= 0 || height <= 0 {
        return;
    }
    for _ in 0..PASSES {
        box_blur_horizontal(data, width, height, stride, radius);
        box_blur_vertical(data, width, height, stride, radius);
    }
}

/// One horizontal sliding-window pass, row by row.
fn box_blur_horizontal(data: &mut [u8], width: i32, height: i32, stride: i32, radius: i32) {
    let window = 2 * radius + 1;
    let mut temp = vec![0u8; width as usize * 4];

    for y in 0..height {
        let row_start = (y * stride) as usize;
        let row = &data[row_start..row_start + width as usize * 4];

        for channel in 0..4usize {
            let mut sum: i32 = 0;
            for dx in -radius..=radius {
                let x = dx.clamp(0, width - 1) as usize;
                sum += row[x * 4 + channel] as i32;
            }
            for x in 0..width {
                temp[x as usize * 4 + channel] = ((sum + window / 2) / window) as u8;
                let entering = (x + radius + 1).clamp(0, width - 1) as usize;
                let leaving = (x - radius).clamp(0, width - 1) as usize;
                sum += row[entering * 4 + channel] as i32;
                sum -= row[leaving * 4 + channel] as i32;
            }
        }

        data[row_start..row_start + width as usize * 4].copy_from_slice(&temp);
    }
}

/// One vertical sliding-window pass, column by column.
fn box_blur_vertical(data: &mut [u8], width: i32, height: i32, stride: i32, radius: i32) {
    let window = 2 * radius + 1;
    let mut temp = vec![0u8; height as usize * 4];

    for x in 0..width {
        for channel in 0..4usize {
            let mut sum: i32 = 0;
            for dy in -radius..=radius {
                let y = dy.clamp(0, height - 1);
                sum += data[(y * stride) as usize + x as usize * 4 + channel] as i32;
            }
            for y in 0..height {
                temp[y as usize * 4 + channel] = ((sum + window / 2) / window) as u8;
                let entering = (y + radius + 1).clamp(0, height - 1);
                let leaving = (y - radius).clamp(0, height - 1);
                sum += data[(entering * stride) as usize + x as usize * 4 + channel] as i32;
                sum -= data[(leaving * stride) as usize + x as usize * 4 + channel] as i32;
            }
        }
        for y in 0..height {
            let idx = (y * stride) as usize + x as usize * 4;
            data[idx..idx + 4].copy_from_slice(&temp[y as usize * 4..y as usize * 4 + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single fully-opaque white pixel at the center of an otherwise
    /// transparent buffer, alpha channel at index 3 (matching Cairo's
    /// native-endian ARGB32 byte order on a little-endian target: B, G,
    /// R, A — see `render::tests::read_pixel`'s comment).
    fn single_pixel_buffer(size: i32) -> Vec<u8> {
        let mut data = vec![0u8; (size * size * 4) as usize];
        let center = size / 2;
        let idx = (center * size * 4 + center * 4) as usize;
        data[idx..idx + 4].copy_from_slice(&[255, 255, 255, 255]);
        data
    }

    #[test]
    fn zero_radius_is_a_no_op() {
        let mut data = single_pixel_buffer(21);
        let before = data.clone();
        box_blur(&mut data, 21, 21, 21 * 4, 0.0);
        assert_eq!(data, before);
    }

    #[test]
    fn blur_spreads_a_single_pixel_to_its_neighbors() {
        let size = 21;
        let mut data = single_pixel_buffer(size);
        box_blur(&mut data, size, size, size * 4, 3.0);

        let center = size / 2;
        let at = |x: i32, y: i32| -> u8 { data[(y * size * 4 + x * 4 + 3) as usize] };

        // The center pixel is no longer the only non-zero one -- energy
        // spread to a direct neighbor -- but it also lost some of its own
        // original intensity to that spreading.
        assert!(at(center, center) > 0);
        assert!(at(center, center) < 255);
        assert!(at(center + 1, center) > 0);
        // Far enough away, nothing reaches it.
        assert_eq!(at(0, 0), 0);
    }

    /// A filled square in the center of an otherwise transparent buffer —
    /// representative of the real use case (a shadow's filled rounded
    /// rect), unlike a single isolated pixel: local window sums stay large
    /// relative to the window size throughout every pass, so integer
    /// rounding can't compound into a near-total loss the way it would
    /// for a lone bright point spread thinner with every pass.
    fn filled_block_buffer(size: i32, block: i32) -> Vec<u8> {
        let mut data = vec![0u8; (size * size * 4) as usize];
        let start = (size - block) / 2;
        for y in start..start + block {
            for x in start..start + block {
                let idx = (y * size * 4 + x * 4) as usize;
                data[idx..idx + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        data
    }

    #[test]
    fn blur_preserves_total_energy_approximately_for_a_filled_shape() {
        // A box blur (unlike a naive edge-dropping one) conserves the sum
        // of the buffer as long as it never reads past a clamped edge --
        // true here since the block is centered, far from the buffer's
        // edges relative to the radius.
        let size = 61;
        let mut data = filled_block_buffer(size, 15);
        let total_before: u64 = data.iter().map(|&b| b as u64).sum();

        box_blur(&mut data, size, size, size * 4, 4.0);
        let total_after: u64 = data.iter().map(|&b| b as u64).sum();

        // Integer rounding in the sliding-window division loses a little
        // energy each pass; allow a small tolerance rather than requiring
        // an exact match.
        let diff = total_before.abs_diff(total_after);
        assert!(diff < total_before / 20, "expected near-conservation: before={total_before} after={total_after}");
    }
}
