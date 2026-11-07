//! Pixel buffers and the operations on them.
//!
//! Every buffer in the app is **BGRA, 8 bits per channel, premultiplied, rows top-down with no
//! padding**. That is what a 32-bit GDI DIB holds, so captures need no conversion. Screen pixels
//! are opaque, which makes premultiplied and straight alpha the same for them. tiny-skia expects
//! RGBA but only ever treats the channels symmetrically, so the renderer swaps red and blue in
//! the colours it paints with, and PNG export swaps them back once.

use crate::geom::IRect;

#[derive(Clone, Debug)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Image {
    pub fn new(w: u32, h: u32) -> Image {
        Image { w, h, data: vec![0; w as usize * h as usize * 4] }
    }
    pub fn from_bgra(w: u32, h: u32, data: Vec<u8>) -> Option<Image> {
        (data.len() == w as usize * h as usize * 4).then_some(Image { w, h, data })
    }
    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.w as i32, self.h as i32)
    }
    pub fn stride(&self) -> usize {
        self.w as usize * 4
    }
    /// The pixel at (x, y) as (r, g, b), or `None` outside the image.
    pub fn rgb_at(&self, x: i32, y: i32) -> Option<(u8, u8, u8)> {
        if !self.bounds().contains(x, y) {
            return None;
        }
        let i = (y as usize * self.w as usize + x as usize) * 4;
        Some((self.data[i + 2], self.data[i + 1], self.data[i]))
    }

    /// Copy of the part of the image inside `r`, clipped to the image.
    pub fn crop(&self, r: IRect) -> Image {
        let r = r.intersect(&self.bounds());
        let mut out = Image::new(r.w.max(0) as u32, r.h.max(0) as u32);
        copy_rect(&self.data, self.w, r, &mut out.data, out.w, 0, 0);
        out
    }

    /// A darkened copy, for the area outside the selection. `keep` is out of 256.
    pub fn dimmed(&self, keep: u16) -> Image {
        let mut out = self.clone();
        dim_in_place(&mut out.data, keep);
        out
    }
}

/// Multiply colour channels by `keep / 256`, leaving alpha alone.
pub fn dim_in_place(px: &mut [u8], keep: u16) {
    for p in px.chunks_exact_mut(4) {
        p[0] = ((p[0] as u16 * keep) >> 8) as u8;
        p[1] = ((p[1] as u16 * keep) >> 8) as u8;
        p[2] = ((p[2] as u16 * keep) >> 8) as u8;
    }
}

/// Copy the pixels of `src_rect` (in source coordinates, already inside the source) into `dst`
/// with its top-left at (dx, dy). Rows that fall outside `dst` are skipped.
pub fn copy_rect(src: &[u8], src_w: u32, src_rect: IRect, dst: &mut [u8], dst_w: u32, dx: i32, dy: i32) {
    if src_rect.is_empty() || src_w == 0 || dst_w == 0 {
        return;
    }
    let dst_h = (dst.len() / (dst_w as usize * 4)) as i32;
    let src_h = (src.len() / (src_w as usize * 4)) as i32;
    // Clip against both buffers so callers can pass rough rectangles.
    let mut r = src_rect.intersect(&IRect::new(0, 0, src_w as i32, src_h));
    let (mut dx, mut dy) = (dx + (r.x - src_rect.x), dy + (r.y - src_rect.y));
    if dx < 0 {
        r.x -= dx;
        r.w += dx;
        dx = 0;
    }
    if dy < 0 {
        r.y -= dy;
        r.h += dy;
        dy = 0;
    }
    r.w = r.w.min(dst_w as i32 - dx);
    r.h = r.h.min(dst_h - dy);
    if r.is_empty() {
        return;
    }
    let row = r.w as usize * 4;
    for i in 0..r.h as usize {
        let s = ((r.y as usize + i) * src_w as usize + r.x as usize) * 4;
        let d = ((dy as usize + i) * dst_w as usize + dx as usize) * 4;
        dst[d..d + row].copy_from_slice(&src[s..s + row]);
    }
}

/// Approximate Gaussian blur of the pixels inside `r`: three box blurs of the given radius.
/// Edge pixels are clamped, so the region does not darken at its border.
pub fn blur_region(px: &mut [u8], w: u32, h: u32, r: IRect, radius: u32) {
    let r = r.intersect(&IRect::new(0, 0, w as i32, h as i32));
    if r.is_empty() || radius == 0 {
        return;
    }
    let (rw, rh) = (r.w as usize, r.h as usize);
    let mut a = vec![0u8; rw * rh * 4];
    copy_rect(px, w, r, &mut a, rw as u32, 0, 0);
    let mut b = vec![0u8; a.len()];
    for _ in 0..3 {
        box_pass(&a, &mut b, rw, rh, radius as usize, true);
        box_pass(&b, &mut a, rw, rh, radius as usize, false);
    }
    copy_rect(&a, rw as u32, IRect::new(0, 0, rw as i32, rh as i32), px, w, r.x, r.y);
}

/// One sliding-window box blur along rows (`horizontal`) or columns.
fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, radius: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let win = (2 * radius + 1) as u32;
    for line in 0..lines {
        let mut sum = [0u32; 4];
        // Prime the window centred on pixel 0, clamping indices at the ends.
        for k in 0..win as usize {
            let i = (k as isize - radius as isize).clamp(0, len as isize - 1) as usize;
            let p = idx(line, i);
            for c in 0..4 {
                sum[c] += src[p + c] as u32;
            }
        }
        for i in 0..len {
            let o = idx(line, i);
            for c in 0..4 {
                dst[o + c] = ((sum[c] + win / 2) / win) as u8;
            }
            let out_i = (i as isize - radius as isize).clamp(0, len as isize - 1) as usize;
            let in_i = (i + radius + 1).min(len - 1);
            let (po, pi) = (idx(line, out_i), idx(line, in_i));
            for c in 0..4 {
                sum[c] = sum[c] + src[pi + c] as u32 - src[po + c] as u32;
            }
        }
    }
}

/// Replace each `block`×`block` cell inside `r` with its average colour. Cells are aligned to
/// the image, not to `r`, so overlapping pixelate regions line up.
pub fn pixelate_region(px: &mut [u8], w: u32, h: u32, r: IRect, block: u32) {
    let r = r.intersect(&IRect::new(0, 0, w as i32, h as i32));
    if r.is_empty() || block < 2 {
        return;
    }
    let b = block as i32;
    let mut cy = r.y.div_euclid(b) * b;
    while cy < r.bottom() {
        let mut cx = r.x.div_euclid(b) * b;
        while cx < r.right() {
            let cell = IRect::new(cx, cy, b, b).intersect(&r);
            let mut sum = [0u32; 4];
            for y in cell.y..cell.bottom() {
                for x in cell.x..cell.right() {
                    let p = (y as usize * w as usize + x as usize) * 4;
                    for c in 0..4 {
                        sum[c] += px[p + c] as u32;
                    }
                }
            }
            let n = (cell.w * cell.h).max(1) as u32;
            let avg = [sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n];
            for y in cell.y..cell.bottom() {
                for x in cell.x..cell.right() {
                    let p = (y as usize * w as usize + x as usize) * 4;
                    for c in 0..4 {
                        px[p + c] = avg[c] as u8;
                    }
                }
            }
            cx += b;
        }
        cy += b;
    }
}

/// Encode as an 8-bit RGB PNG (screenshots are opaque, so alpha would only add size).
pub fn encode_png(img: &Image) -> Result<Vec<u8>, String> {
    let mut rgb = Vec::with_capacity(img.w as usize * img.h as usize * 3);
    for p in img.data.chunks_exact(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let mut out = Vec::with_capacity(rgb.len() / 3);
    {
        let mut enc = png::Encoder::new(&mut out, img.w, img.h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                let i = ((y * w + x) * 4) as usize;
                img.data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        img
    }

    #[test]
    fn crop_clips_to_image() {
        let img = checker(10, 10);
        let c = img.crop(IRect::new(8, 8, 5, 5));
        assert_eq!((c.w, c.h), (2, 2));
        assert_eq!(c.rgb_at(0, 0), img.rgb_at(8, 8));
    }

    #[test]
    fn copy_rect_clips_negative_destination() {
        let src = checker(4, 4);
        let mut dst = vec![7u8; 4 * 4 * 4];
        copy_rect(&src.data, 4, IRect::new(0, 0, 4, 4), &mut dst, 4, -2, -2);
        // Source (2, 2) lands on destination (0, 0); the rest stays untouched.
        assert_eq!(&dst[0..4], &src.data[(2 * 4 + 2) * 4..(2 * 4 + 2) * 4 + 4]);
        assert_eq!(&dst[(3 * 4 + 3) * 4..(3 * 4 + 3) * 4 + 4], &[7, 7, 7, 7]);
    }

    #[test]
    fn blur_flattens_checker_and_stays_inside() {
        let mut img = checker(40, 40);
        let before = img.clone();
        blur_region(&mut img.data, 40, 40, IRect::new(10, 10, 20, 20), 3);
        let (r, _, _) = img.rgb_at(20, 20).unwrap();
        assert!((110..=145).contains(&r), "centre should be near grey, got {r}");
        assert_eq!(img.rgb_at(5, 5), before.rgb_at(5, 5));
        assert_eq!(img.rgb_at(30, 30), before.rgb_at(30, 30));
    }

    #[test]
    fn blur_keeps_flat_colour() {
        let mut img = Image::new(16, 16);
        img.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[10, 200, 30, 255]));
        blur_region(&mut img.data, 16, 16, IRect::new(0, 0, 16, 16), 4);
        assert!(img.data.chunks_exact(4).all(|p| p == [10, 200, 30, 255]));
    }

    #[test]
    fn pixelate_averages_cells() {
        let mut img = checker(8, 8);
        pixelate_region(&mut img.data, 8, 8, IRect::new(0, 0, 8, 8), 4);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(img.rgb_at(x, y), Some((127, 127, 127)));
            }
        }
    }

    #[test]
    fn png_has_signature_and_swaps_channels() {
        let mut img = Image::new(2, 1);
        img.data.copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
        let png = encode_png(&img).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let dec = png::Decoder::new(std::io::Cursor::new(png));
        let mut rd = dec.read_info().unwrap();
        let mut buf = vec![0; rd.output_buffer_size().unwrap()];
        let info = rd.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..info.buffer_size()], &[3, 2, 1, 6, 5, 4]);
    }

    #[test]
    fn dim_scales_colour_not_alpha() {
        let mut px = vec![200, 100, 50, 255];
        dim_in_place(&mut px, 128);
        assert_eq!(px, vec![100, 50, 25, 255]);
    }
}
