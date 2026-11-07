//! Text recognition with the OCR engine built into Windows (`Windows.Media.Ocr`). Nothing to
//! download: it uses the language packs already installed for the user's display languages.
//!
//! Runs on a worker thread; WinRT is only loaded the first time someone grabs text.

use prism_core::Image;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

/// Screen text is often 11–14 px tall, which is below what the Windows engine reads reliably.
/// Small captures are scaled up first; it costs a few milliseconds and fixes most misreads.
fn prepare(img: &Image, max_dim: u32) -> Image {
    let longest = img.w.max(img.h);
    let scale = if longest * 3 <= max_dim.min(2400) {
        3
    } else if longest * 2 <= max_dim.min(4000) {
        2
    } else {
        1
    };
    // A margin helps with text that touches the edge of the selection.
    let pad = 12 * scale;
    let (w, h) = (img.w * scale + 2 * pad, img.h * scale + 2 * pad);
    let mut out = Image::new(w, h);
    let sw = img.w as i64;
    let sh = img.h as i64;
    for y in 0..h {
        // Sample with bilinear filtering; padding clamps to the edge pixels.
        let fy = ((y as f32 - pad as f32 + 0.5) / scale as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as i64;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for x in 0..w {
            let fx = ((x as f32 - pad as f32 + 0.5) / scale as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as i64;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            let px = |xx: i64, yy: i64, c: usize| img.data[((yy * sw + xx) * 4) as usize + c] as f32;
            let o = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
                let bot = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
                out.data[o + c] = (top * (1.0 - ty) + bot * ty).round() as u8;
            }
            out.data[o + 3] = 255;
        }
    }
    out
}

pub fn recognize(img: &Image) -> Result<String, String> {
    if img.w == 0 || img.h == 0 {
        return Ok(String::new());
    }
    unsafe {
        // Fine if the thread already has an apartment.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| "Windows has no OCR language installed. Add a language with OCR support in Settings > Time & language.".to_string())?;
    let max_dim = OcrEngine::MaxImageDimension().unwrap_or(2600);
    let prepared = prepare(img, max_dim);
    if prepared.w > max_dim || prepared.h > max_dim {
        return Err(format!("The selection is too large to read (limit {max_dim} px)."));
    }
    let buffer = CryptographicBuffer::CreateFromByteArray(&prepared.data).map_err(|e| e.message())?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, prepared.w as i32, prepared.h as i32)
        .map_err(|e| e.message())?;
    let result = engine.RecognizeAsync(&bitmap).map_err(|e| e.message())?.join().map_err(|e| e.message())?;
    let lines = result.Lines().map_err(|e| e.message())?;
    let mut out = Vec::new();
    for i in 0..lines.Size().map_err(|e| e.message())? {
        if let Ok(line) = lines.GetAt(i) {
            if let Ok(t) = line.Text() {
                out.push(t.to_string());
            }
        }
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_scales_small_and_pads() {
        let mut img = Image::new(10, 5);
        img.data.fill(200);
        let p = prepare(&img, 10_000);
        assert_eq!((p.w, p.h), (10 * 3 + 72, 5 * 3 + 72));
        assert!(p.data.chunks_exact(4).all(|px| px == [200, 200, 200, 255]));
        let big = Image::new(3000, 100);
        let p = prepare(&big, 10_000);
        assert_eq!(p.w, 3000 + 24);
    }
}
