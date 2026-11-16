//! Cuts a reference sheet (several views of one subject side by side on a plain background) into one image per view,
//! which is what Tripo's multiview endpoint takes.
//!
//! The background color is read from the corners. A column counts as part of a view when enough of its pixels differ
//! from the background; runs of such columns, split by background gaps, are the views.

use image::{Rgba, RgbaImage, imageops};

/// A pixel differs from the background when its summed RGB difference is over this.
const COLOR_THRESHOLD: u32 = 60;
/// Each crop gets this much background around the subject, as a share of its larger side.
const PADDING: f32 = 0.08;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    /// Exclusive.
    pub end: u32,
}

fn background(img: &RgbaImage) -> Rgba<u8> {
    let (w, h) = img.dimensions();
    let patch = (w.min(h) / 50).max(1);
    let mut sum = [0u64; 4];
    let mut n = 0u64;
    for (cx, cy) in [
        (0, 0),
        (w - patch, 0),
        (0, h - patch),
        (w - patch, h - patch),
    ] {
        for y in cy..cy + patch {
            for x in cx..cx + patch {
                let p = img.get_pixel(x, y);
                for c in 0..4 {
                    sum[c] += p[c] as u64;
                }
                n += 1;
            }
        }
    }
    Rgba(sum.map(|s| (s / n) as u8))
}

fn differs(p: &Rgba<u8>, bg: &Rgba<u8>) -> bool {
    // Transparent pixels on a transparent background are background, whatever their color.
    if p[3] < 16 && bg[3] < 16 {
        return false;
    }
    let d: u32 = (0..3)
        .map(|c| (p[c] as i32 - bg[c] as i32).unsigned_abs())
        .sum();
    d > COLOR_THRESHOLD || (p[3] as i32 - bg[3] as i32).abs() > 64
}

/// Runs of `true` in `occupied`, with gaps narrower than `min_gap` bridged and runs narrower than `min_len` dropped.
fn runs(occupied: &[bool], min_gap: u32, min_len: u32) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut start = None;
    for (i, &on) in occupied.iter().chain(std::iter::once(&false)).enumerate() {
        let i = i as u32;
        match (on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                match spans.last_mut() {
                    Some(last) if s - last.end < min_gap => last.end = i,
                    _ => spans.push(Span { start: s, end: i }),
                }
                start = None;
            }
            _ => {}
        }
    }
    spans.retain(|s| s.end - s.start >= min_len);
    spans
}

/// The column spans of the views in a sheet, left to right.
pub fn find_views(img: &RgbaImage) -> Vec<Span> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return vec![];
    }
    let bg = background(img);
    // Ignore a few stray pixels (noise, JPEG artifacts) per column.
    let min_pixels = (h / 200).max(2);
    let occupied: Vec<bool> = (0..w)
        .map(|x| {
            (0..h)
                .filter(|&y| differs(img.get_pixel(x, y), &bg))
                .count() as u32
                >= min_pixels
        })
        .collect();
    runs(&occupied, (w / 100).max(2), (w / 40).max(4))
}

/// Crops one view: the subject's bounding box in `span`, centered on a square of background with some padding.
pub fn crop_view(img: &RgbaImage, span: Span) -> RgbaImage {
    let (_, h) = img.dimensions();
    let bg = background(img);
    let rows: Vec<u32> = (0..h)
        .filter(|&y| (span.start..span.end).any(|x| differs(img.get_pixel(x, y), &bg)))
        .collect();
    let (top, bottom) = match (rows.first(), rows.last()) {
        (Some(&t), Some(&b)) => (t, b + 1),
        _ => (0, h),
    };
    let (cw, ch) = (span.end - span.start, bottom - top);
    let side = ((cw.max(ch) as f32) * (1.0 + 2.0 * PADDING)).round() as u32;
    let mut out = RgbaImage::from_pixel(side, side, bg);
    let part = imageops::crop_imm(img, span.start, top, cw, ch).to_image();
    imageops::replace(
        &mut out,
        &part,
        ((side - cw) / 2) as i64,
        ((side - ch) / 2) as i64,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(figures: &[(u32, u32)]) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(400, 200, Rgba([200, 200, 200, 255]));
        for &(x0, x1) in figures {
            for x in x0..x1 {
                for y in 40..180 {
                    img.put_pixel(x, y, Rgba([90, 40, 30, 255]));
                }
            }
        }
        img
    }

    #[test]
    fn finds_each_view() {
        let img = sheet(&[(20, 80), (120, 170), (220, 290), (320, 380)]);
        let spans = find_views(&img);
        assert_eq!(
            spans,
            [
                Span { start: 20, end: 80 },
                Span {
                    start: 120,
                    end: 170
                },
                Span {
                    start: 220,
                    end: 290
                },
                Span {
                    start: 320,
                    end: 380
                },
            ]
        );
        let crop = crop_view(&img, spans[0]);
        // 140 px tall subject + 8% padding on each side.
        assert_eq!(crop.dimensions(), (162, 162));
        assert_eq!(crop.get_pixel(81, 81), &Rgba([90, 40, 30, 255]));
        assert_eq!(crop.get_pixel(2, 2), &Rgba([200, 200, 200, 255]));
    }

    #[test]
    fn bridges_small_gaps_and_drops_specks() {
        // A 2 px gap inside one figure, and a 3 px speck.
        let img = sheet(&[(20, 60), (62, 100), (200, 203), (250, 330)]);
        assert_eq!(
            find_views(&img),
            [
                Span {
                    start: 20,
                    end: 100
                },
                Span {
                    start: 250,
                    end: 330
                }
            ]
        );
    }

    #[test]
    fn empty_image_has_no_views() {
        assert!(find_views(&sheet(&[])).is_empty());
    }
}
