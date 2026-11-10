//! Snapping a highlighter stroke to what's under it.
//!
//! A stroke dragged over text becomes one box per line it crosses, each fitted to the ink of
//! that line and widened to whole words. It works on pixels alone, so it fits buttons, icons and
//! other things that stand out from their background as well as text, and needs no OCR.
//!
//! The steps:
//! 1. The background is the commonest colour in the band the stroke covers; anything that
//!    differs from it clearly is ink.
//! 2. Rows with ink under the stroke's span form runs, and each run is a line. Runs with tiny
//!    gaps between them (the dot of an i, an accent) are one line.
//! 3. Each line the stroke's centre crosses gets a box from its first to its last ink column
//!    under the stroke, carried out to the ends of the words at either end.
//!
//! When nothing fits (a photo, an empty area), there's no snap and the stroke stays as drawn.

use crate::geom::{FRect, IRect, Pt};
use crate::image::Image;

/// The tallest line that's still snapped to, in image pixels. Anything taller is a picture.
const MAX_LINE: i32 = 120;
/// A line at least this tall with more colours than `PICTURE_COLORS` is a picture, not text or
/// a button. Text is a colour or two plus its anti-aliasing; small icons and thumbnails beside
/// text stay under the height.
const PICTURE_H: i32 = 40;
const PICTURE_COLORS: usize = 400;
/// How far a channel must differ from the background to count as ink, out of 255. Faint lines
/// and gradients stay below it.
const INK: i32 = 48;
/// More lines than one stroke could mean.
const MAX_LINES: usize = 60;

/// Boxes to highlight for a highlighter stroke through `pts` over `img`, kept inside `clip`.
/// `brush` is the stroke's width. None means nothing to snap to.
pub fn highlight_boxes(img: &Image, clip: IRect, pts: &[Pt], brush: f32) -> Option<Vec<FRect>> {
    let clip = clip.intersect(&img.bounds());
    if clip.is_empty() || pts.is_empty() {
        return None;
    }
    let span = FRect::of_points(pts);
    let half = brush / 2.0;
    let x0 = (span.l.round() as i32).clamp(clip.x, clip.right() - 1);
    let x1 = (span.r.round() as i32).clamp(clip.x, clip.right() - 1);
    let px = Pixels { img };

    // 1. The background, from around the band the stroke paints. Wider than the stroke, since a
    //    short stroke or a click can sit almost wholly on ink.
    let m = brush * 2.0;
    let bx0 = ((span.l - m).floor() as i32).max(clip.x);
    let bx1 = ((span.r + m).ceil() as i32).min(clip.right() - 1);
    let by0 = ((span.t - m).floor() as i32).max(clip.y);
    let by1 = ((span.b + m).ceil() as i32).min(clip.bottom() - 1);
    let bg = background(&px, bx0, bx1, by0, by1)?;
    let ink = |x: i32, y: i32| px.differs(x, y, bg);

    // 2. Lines: runs of rows with ink between x0 and x1.
    let wy0 = (span.t.floor() as i32 - MAX_LINE).max(clip.y);
    let wy1 = (span.b.ceil() as i32 + MAX_LINE).min(clip.bottom() - 1);
    let mut runs: Vec<(i32, i32)> = Vec::new();
    for y in wy0..=wy1 {
        if px.row_has_ink(y, x0, x1, bg) {
            match runs.last_mut() {
                Some(last) if last.1 == y => last.1 = y + 1,
                _ => runs.push((y, y + 1)),
            }
        }
    }
    let lines = merge_close(runs);

    // The lines the stroke's centre passes through, or else the nearest within its width.
    let (cy0, cy1) = (span.t - 2.0, span.b + 2.0);
    let mut chosen: Vec<(i32, i32)> = lines.iter().copied().filter(|&(a, b)| (b as f32) > cy0 && (a as f32) <= cy1).collect();
    if chosen.is_empty() {
        let cy = (span.t + span.b) / 2.0;
        let gap = |&(a, b): &(i32, i32)| if cy < a as f32 { a as f32 - cy } else { cy - b as f32 }.max(0.0);
        chosen.extend(lines.iter().filter(|l| gap(l) <= half).min_by(|a, b| gap(a).total_cmp(&gap(b))));
    }
    if chosen.is_empty() || chosen.len() > MAX_LINES {
        return None;
    }

    // 3. A box per line.
    let mut boxes = Vec::with_capacity(chosen.len());
    for (top, bottom) in chosen {
        let h = bottom - top;
        if h >= MAX_LINE {
            return None;
        }
        if h < 3 {
            continue;
        }
        let col = |x: i32| (top..bottom).any(|y| ink(x, y));
        let (Some(first), Some(last)) = ((x0..=x1).find(|&x| col(x)), (x0..=x1).rev().find(|&x| col(x))) else { continue };
        // A space is wider than the gap between letters; stop at the first one.
        let word_gap = ((h as f32 * 0.3).round() as i32).max(3);
        let l = extend(first, -1, clip.x, word_gap, &col);
        let r = extend(last, 1, clip.right() - 1, word_gap, &col);
        if h >= PICTURE_H && px.colors(l, r, top, bottom) > PICTURE_COLORS {
            return None;
        }
        let pad_x = (h as f32 * 0.12).clamp(1.0, 4.0);
        let pad_y = (h as f32 * 0.18).clamp(1.0, 5.0);
        let b = FRect { l: l as f32 - pad_x, t: top as f32 - pad_y, r: (r + 1) as f32 + pad_x, b: bottom as f32 + pad_y };
        let c = FRect { l: clip.x as f32, t: clip.y as f32, r: clip.right() as f32, b: clip.bottom() as f32 };
        boxes.push(FRect { l: b.l.max(c.l), t: b.t.max(c.t), r: b.r.min(c.r), b: b.b.min(c.b) });
    }
    (!boxes.is_empty()).then_some(boxes)
}

struct Pixels<'a> {
    img: &'a Image,
}

impl Pixels<'_> {
    /// BGR of an in-bounds pixel.
    fn at(&self, x: i32, y: i32) -> [u8; 3] {
        let i = (y as usize * self.img.w as usize + x as usize) * 4;
        [self.img.data[i], self.img.data[i + 1], self.img.data[i + 2]]
    }

    /// How many different colours, at 16 levels a channel, are in columns x0..=x1 of rows
    /// y0..y1.
    fn colors(&self, x0: i32, x1: i32, y0: i32, y1: i32) -> usize {
        let mut seen = vec![false; 4096];
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..=x1 {
                let p = self.at(x, y);
                let k = bucket(p);
                if !seen[k] {
                    seen[k] = true;
                    n += 1;
                }
            }
        }
        n
    }

    /// Whether any pixel in columns x0..=x1 of row y differs from `bg`.
    fn row_has_ink(&self, y: i32, x0: i32, x1: i32, bg: [u8; 3]) -> bool {
        let row = y as usize * self.img.w as usize;
        let px = &self.img.data[(row + x0 as usize) * 4..(row + x1 as usize + 1) * 4];
        // Most of a page is exactly the background colour: check for that first, cheaply.
        let same = u32::from_le_bytes([bg[0], bg[1], bg[2], 0]);
        px.chunks_exact(4).any(|p| {
            u32::from_le_bytes([p[0], p[1], p[2], 0]) != same && (0..3).any(|c| (p[c] as i32 - bg[c] as i32).abs() > INK)
        })
    }

    fn differs(&self, x: i32, y: i32, bg: [u8; 3]) -> bool {
        let p = self.at(x, y);
        (0..3).any(|c| (p[c] as i32 - bg[c] as i32).abs() > INK)
    }
}

/// A colour's box when each channel is cut to 16 levels: close enough to group anti-aliasing
/// and noise, apart enough for text.
fn bucket(p: [u8; 3]) -> usize {
    (p[0] as usize >> 4) << 8 | (p[1] as usize >> 4) << 4 | p[2] as usize >> 4
}

/// The commonest colour in the rectangle, averaged over the pixels near it.
fn background(px: &Pixels, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<[u8; 3]> {
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let mut count = vec![0u32; 4096];
    let mut sum = vec![[0u32; 3]; 4096];
    // About 40,000 samples is plenty, however big the area.
    let area = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f32;
    let step = ((area / 40_000.0).sqrt().ceil() as usize).max(1);
    for y in (y0..=y1).step_by(step) {
        for x in (x0..=x1).step_by(step) {
            let p = px.at(x, y);
            let k = bucket(p);
            count[k] += 1;
            for c in 0..3 {
                sum[k][c] += p[c] as u32;
            }
        }
    }
    let (k, n) = count.iter().enumerate().max_by_key(|(_, n)| **n)?;
    (*n > 0).then(|| sum[k].map(|s| (s / n) as u8))
}

/// Join runs split by gaps small for their size, like the dot above an i.
fn merge_close(runs: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(runs.len());
    for r in runs {
        if let Some(last) = out.last_mut() {
            let tallest = (last.1 - last.0).max(r.1 - r.0);
            if r.0 - last.1 <= (tallest / 4).max(1) {
                last.1 = r.1;
                continue;
            }
        }
        out.push(r);
    }
    out
}

/// From ink column `from`, walk in direction `dir` towards `limit` over gaps narrower than
/// `gap`, and return the last ink column reached.
fn extend(from: i32, dir: i32, limit: i32, gap: i32, col: &dyn Fn(i32) -> bool) -> i32 {
    let (mut x, mut end, mut blank) = (from, from, 0);
    while x != limit {
        x += dir;
        if col(x) {
            end = x;
            blank = 0;
        } else {
            blank += 1;
            if blank >= gap {
                break;
            }
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white page with black blocks standing in for letters.
    fn page(w: u32, h: u32, blocks: &[(i32, i32, i32, i32)]) -> Image {
        let mut img = Image::new(w, h);
        img.data.fill(255);
        for &(x, y, bw, bh) in blocks {
            for yy in y..y + bh {
                for xx in x..x + bw {
                    let i = (yy as usize * w as usize + xx as usize) * 4;
                    img.data[i..i + 3].fill(0);
                }
            }
        }
        img
    }

    /// A word of `n` 6×12 letters with 1px between them, starting at x, top at y.
    fn word(x: i32, y: i32, n: i32) -> Vec<(i32, i32, i32, i32)> {
        (0..n).map(|i| (x + i * 7, y, 6, 12)).collect()
    }

    fn stroke(x0: f32, x1: f32, y: f32) -> Vec<Pt> {
        vec![Pt::new(x0, y), Pt::new(x1, y + 1.0)]
    }

    #[test]
    fn snaps_to_the_line_and_whole_words() {
        // "aaaa bbbb cccc" on one line at y 20..32; words 8px apart.
        let mut blocks = word(10, 20, 4);
        blocks.extend(word(46, 20, 4));
        blocks.extend(word(82, 20, 4));
        let img = page(200, 80, &blocks);
        // From the middle of the first word to the middle of the second.
        let boxes = highlight_boxes(&img, img.bounds(), &stroke(20.0, 55.0, 27.0), 16.0).unwrap();
        assert_eq!(boxes.len(), 1);
        let b = boxes[0];
        assert!(b.l > 6.0 && b.l <= 10.0, "starts at the first word: {b:?}");
        assert!(b.r >= 73.0 && b.r < 78.0, "ends with the second word: {b:?}");
        assert!(b.t > 15.0 && b.t <= 20.0 && b.b >= 32.0 && b.b < 37.0, "fits the line: {b:?}");
    }

    #[test]
    fn a_click_picks_the_word_under_it() {
        let mut blocks = word(10, 20, 4);
        blocks.extend(word(46, 20, 4));
        let img = page(120, 60, &blocks);
        let b = highlight_boxes(&img, img.bounds(), &[Pt::new(50.0, 26.0)], 16.0).unwrap()[0];
        assert!(b.l > 40.0 && b.l <= 46.0 && b.r >= 73.0 && b.r < 78.0, "{b:?}");
    }

    #[test]
    fn a_stroke_down_three_lines_gets_three_boxes() {
        let mut blocks = word(10, 10, 5);
        blocks.extend(word(10, 30, 5));
        blocks.extend(word(10, 50, 5));
        blocks.extend(word(10, 70, 5));
        let img = page(100, 100, &blocks);
        let boxes = highlight_boxes(&img, img.bounds(), &[Pt::new(20.0, 14.0), Pt::new(30.0, 56.0)], 14.0).unwrap();
        assert_eq!(boxes.len(), 3);
        assert!(boxes.windows(2).all(|w| w[0].b <= w[1].t), "separate lines: {boxes:?}");
    }

    #[test]
    fn the_dot_of_an_i_stays_with_its_line() {
        let mut blocks = word(10, 24, 3);
        blocks.push((17, 20, 6, 2)); // a dot 2px above the second letter
        let img = page(80, 60, &blocks);
        let boxes = highlight_boxes(&img, img.bounds(), &stroke(12.0, 28.0, 30.0), 16.0).unwrap();
        assert_eq!(boxes.len(), 1);
        assert!(boxes[0].t <= 20.0, "{boxes:?}");
    }

    #[test]
    fn works_on_a_dark_background() {
        let mut img = page(100, 50, &word(10, 20, 4));
        for p in img.data.chunks_exact_mut(4) {
            p[..3].iter_mut().for_each(|c| *c = 255 - *c);
        }
        assert!(highlight_boxes(&img, img.bounds(), &stroke(12.0, 30.0, 26.0), 16.0).is_some());
    }

    #[test]
    fn blank_area_or_picture_does_not_snap() {
        let img = page(100, 100, &[]);
        assert!(highlight_boxes(&img, img.bounds(), &stroke(10.0, 90.0, 50.0), 16.0).is_none());
        // A block taller than any line.
        let img = page(100, 400, &[(10, 10, 80, 380)]);
        assert!(highlight_boxes(&img, img.bounds(), &stroke(10.0, 90.0, 200.0), 16.0).is_none());
    }

    #[test]
    fn a_photo_does_not_snap_but_a_button_does() {
        // A 60px square of noise, like a photo, on white.
        let mut img = page(200, 120, &[]);
        for y in 30..90 {
            for x in 20..80 {
                let i = (y * 200 + x) * 4;
                let n = (x * 7919 + y * 104_729) as u32;
                let h = n.wrapping_mul(2_654_435_761);
                img.data[i..i + 3].copy_from_slice(&[(h >> 8) as u8, (h >> 16) as u8, (h >> 24) as u8]);
            }
        }
        assert!(highlight_boxes(&img, img.bounds(), &stroke(30.0, 70.0, 60.0), 16.0).is_none());
        // A flat button snaps whole.
        let img = page(200, 120, &[(100, 48, 80, 24)]);
        let b = highlight_boxes(&img, img.bounds(), &stroke(120.0, 150.0, 60.0), 16.0).unwrap()[0];
        assert!(b.l <= 100.0 && b.r >= 180.0 && b.t <= 48.0 && b.b >= 72.0, "{b:?}");
    }

    #[test]
    fn boxes_stay_inside_the_selection() {
        let img = page(120, 60, &word(10, 20, 10));
        let clip = IRect::new(30, 0, 40, 60);
        let b = highlight_boxes(&img, clip, &stroke(35.0, 60.0, 26.0), 16.0).unwrap()[0];
        assert!(b.l >= 30.0 && b.r <= 70.0, "{b:?}");
    }
}
