use crate::{
    error::{Error, Result},
    imagebuf::ImageBuffer,
};

const MAX_CANVAS_BYTES: usize = 256 << 20;
const MAX_ROWS: usize = 30_000;
const MIN_OVERLAP: usize = 50;
const MIN_APPEND: usize = 15;
const ACCEPT_DIFF: f32 = 9.0;

#[derive(Debug)]
pub struct Stitcher {
    width: u32,
    canvas: Vec<u8>,
    anchor: i32,
    last_offset: i32,
    full: bool,
    max_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushResult {
    pub added_rows: usize,
    pub placed: bool,
}

impl Stitcher {
    pub fn new(width: u32) -> Self {
        let stride = (width as usize).saturating_mul(4).max(1);
        Self {
            width,
            canvas: Vec::new(),
            anchor: 0,
            last_offset: 0,
            full: false,
            max_rows: (MAX_CANVAS_BYTES / stride).min(MAX_ROWS),
        }
    }

    pub fn rows(&self) -> usize {
        self.canvas.len() / self.stride()
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    pub fn push(&mut self, frame: &ImageBuffer) -> Result<PushResult> {
        self.validate(frame)?;
        if self.full {
            return Ok(PushResult {
                added_rows: 0,
                placed: true,
            });
        }
        if self.canvas.is_empty() {
            let added = self.append_rows(&frame.pixels, 0, frame.height as usize);
            return Ok(PushResult {
                added_rows: added,
                placed: true,
            });
        }

        let Some(position) = self.locate(frame) else {
            return Ok(PushResult {
                added_rows: 0,
                placed: false,
            });
        };
        let frame_height = frame.height as i32;
        let previous_anchor = self.anchor;
        let mut placed_position = position;
        let mut added = 0;
        let bottom_overhang = position + frame_height - self.rows() as i32;
        if bottom_overhang >= MIN_APPEND as i32 {
            let start = frame.height as usize - bottom_overhang as usize;
            added += self.append_rows(&frame.pixels, start, frame.height as usize);
        }
        let top_overhang = -position;
        if top_overhang >= MIN_APPEND as i32 {
            let count = top_overhang as usize;
            added += self.prepend_rows(&frame.pixels, count);
            placed_position += count as i32;
        }
        self.last_offset = position - previous_anchor;
        self.anchor = placed_position;
        Ok(PushResult {
            added_rows: added,
            placed: true,
        })
    }

    pub fn seam_append(&mut self, frame: &ImageBuffer) -> Result<usize> {
        self.validate(frame)?;
        let position = self.rows() as i32;
        let added = self.append_rows(&frame.pixels, 0, frame.height as usize);
        self.anchor = position;
        self.last_offset = 0;
        Ok(added)
    }

    pub fn finish(self) -> Result<ImageBuffer> {
        let height = self.rows() as u32;
        let mut image = ImageBuffer::new(self.width, height)?;
        image.pixels = self.canvas;
        Ok(image)
    }

    fn locate(&self, frame: &ImageBuffer) -> Option<i32> {
        let height = frame.height as i32;
        let min = -(height - MIN_OVERLAP as i32);
        let max = self.rows() as i32 - MIN_OVERLAP as i32;
        if min > max {
            return None;
        }
        let predicted = (self.anchor + self.last_offset).clamp(min, max);
        let mut candidates = Vec::new();
        candidates.push(predicted);
        for distance in 1..=240 {
            let high = predicted + distance;
            let low = predicted - distance;
            if high <= max {
                candidates.push(high);
            }
            if low >= min {
                candidates.push(low);
            }
        }
        for position in (min..=max).step_by(8) {
            candidates.push(position);
        }

        let mut best = None;
        let mut best_diff = f32::MAX;
        for position in candidates {
            let Some(diff) = self.diff_at(frame, position) else {
                continue;
            };
            if diff < best_diff {
                best = Some(position);
                best_diff = diff;
                if diff < 0.25 {
                    break;
                }
            }
        }
        (best_diff <= ACCEPT_DIFF).then_some(best?).or(None)
    }

    fn diff_at(&self, frame: &ImageBuffer, position: i32) -> Option<f32> {
        let canvas_height = self.rows() as i32;
        let frame_height = frame.height as i32;
        let start = 0.max(-position);
        let end = frame_height.min(canvas_height - position);
        if end - start < MIN_OVERLAP as i32 {
            return None;
        }
        let ignore_top = (frame_height / 10).clamp(16, frame_height / 4);
        let ignore_bottom = (frame_height * 8 / 100).clamp(16, frame_height / 4);
        let start = start.max(ignore_top);
        let end = end.min(frame_height - ignore_bottom);
        if end <= start {
            return None;
        }

        let samples = 17usize;
        let bands = [(0.08, 0.32), (0.34, 0.66), (0.68, 0.92)];
        let mut total = 0.0f32;
        let mut active = 0usize;
        let mut count = 0usize;
        let row_step = ((end - start) as usize / 120).max(1);
        for fy in (start as usize..end as usize).step_by(row_step) {
            let cy = (position + fy as i32) as usize;
            for (lo, hi) in bands {
                let x0 = (self.width as f64 * lo) as usize;
                let x1 = ((self.width as f64 * hi) as usize).max(x0 + 1);
                for sample in 0..samples {
                    let x = x0 + (x1 - x0) * sample / samples;
                    let fi = (fy * self.width as usize + x) * 4;
                    let ci = (cy * self.width as usize + x) * 4;
                    let fl = luminance(&frame.pixels[fi..fi + 3]);
                    let cl = luminance(&self.canvas[ci..ci + 3]);
                    let diff = (fl - cl).abs();
                    total += diff;
                    count += 1;
                    if diff > 2.0 {
                        active += 1;
                    }
                }
            }
        }
        if count == 0 {
            return None;
        }
        let average = total / count as f32;
        if active == 0 && average != 0.0 {
            return None;
        }
        Some(average)
    }

    fn append_rows(&mut self, pixels: &[u8], start_row: usize, end_row: usize) -> usize {
        let capacity = self.max_rows.saturating_sub(self.rows());
        let count = (end_row - start_row).min(capacity);
        if count == 0 {
            self.full = true;
            return 0;
        }
        let stride = self.stride();
        self.canvas
            .extend_from_slice(&pixels[start_row * stride..(start_row + count) * stride]);
        if count < end_row - start_row || self.rows() >= self.max_rows {
            self.full = true;
        }
        count
    }

    fn prepend_rows(&mut self, pixels: &[u8], count: usize) -> usize {
        let count = count.min(self.max_rows.saturating_sub(self.rows()));
        if count == 0 {
            self.full = true;
            return 0;
        }
        let stride = self.stride();
        let mut next = Vec::with_capacity((self.rows() + count) * stride);
        next.extend_from_slice(&pixels[..count * stride]);
        next.append(&mut self.canvas);
        self.canvas = next;
        if self.rows() >= self.max_rows {
            self.full = true;
        }
        count
    }

    fn validate(&self, frame: &ImageBuffer) -> Result<()> {
        if frame.width != self.width || frame.height == 0 {
            return Err(Error::InvalidBuffer(
                "scroll frame dimensions changed".into(),
            ));
        }
        Ok(())
    }

    fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

fn luminance(pixel: &[u8]) -> f32 {
    0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width: u32, height: u32) -> ImageBuffer {
        let mut image = ImageBuffer::new(width, height).unwrap();
        let mut random = 0x1234_5678u32;
        for y in 0..height {
            for x in 0..width {
                let i = (y as usize * width as usize + x as usize) * 4;
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let value = (random >> 24) as u8;
                image.pixels[i..i + 4].copy_from_slice(&[
                    value,
                    value.wrapping_mul(3),
                    x as u8,
                    255,
                ]);
            }
        }
        image
    }

    fn frame(page: &ImageBuffer, top: i32, height: i32) -> ImageBuffer {
        page.crop(0, top, page.width as i32, height).unwrap()
    }

    #[test]
    fn stitches_sliding_frames() {
        let page = page(128, 900);
        let mut stitcher = Stitcher::new(128);
        for top in [0, 60, 120, 180, 240] {
            assert!(stitcher.push(&frame(&page, top, 240)).unwrap().placed);
        }
        let image = stitcher.finish().unwrap();
        assert_eq!(image.height, 480);
        assert_eq!(image.pixels, page.pixels[..480 * 128 * 4]);
    }

    #[test]
    fn revisits_do_not_grow_canvas() {
        let page = page(128, 900);
        let mut stitcher = Stitcher::new(128);
        for top in [0, 100, 200, 100, 0, 200] {
            stitcher.push(&frame(&page, top, 240)).unwrap();
        }
        assert_eq!(stitcher.rows(), 440);
    }

    #[test]
    fn scrolling_up_prepends() {
        let page = page(128, 900);
        let mut stitcher = Stitcher::new(128);
        stitcher.push(&frame(&page, 400, 240)).unwrap();
        assert_eq!(
            stitcher.push(&frame(&page, 320, 240)).unwrap().added_rows,
            80
        );
        assert_eq!(stitcher.rows(), 320);
    }

    #[test]
    fn unmatched_frame_is_not_added() {
        let first = page(128, 240);
        let mut second = page(128, 240);
        for pixel in second.pixels.chunks_exact_mut(4) {
            pixel[0] = pixel[0].wrapping_add(80);
        }
        let mut stitcher = Stitcher::new(128);
        stitcher.push(&first).unwrap();
        let result = stitcher.push(&second).unwrap();
        assert!(!result.placed);
        assert_eq!(stitcher.rows(), 240);
    }

    #[test]
    fn fixed_header_is_ignored() {
        let page = page(128, 900);
        let header = page.pixels[..24 * 128 * 4].to_vec();
        let with_header = |top| {
            let mut image = frame(&page, top, 240);
            image.pixels[..header.len()].copy_from_slice(&header);
            image
        };
        let mut stitcher = Stitcher::new(128);
        stitcher.push(&with_header(0)).unwrap();
        assert_eq!(stitcher.push(&with_header(80)).unwrap().added_rows, 80);
        assert_eq!(stitcher.push(&with_header(160)).unwrap().added_rows, 80);
    }

    #[test]
    fn seam_recovers_after_fast_jump() {
        let page = page(128, 1200);
        let mut stitcher = Stitcher::new(128);
        stitcher.push(&frame(&page, 0, 240)).unwrap();
        assert!(!stitcher.push(&frame(&page, 700, 240)).unwrap().placed);
        assert_eq!(stitcher.seam_append(&frame(&page, 700, 240)).unwrap(), 240);
        assert_eq!(
            stitcher.push(&frame(&page, 760, 240)).unwrap().added_rows,
            60
        );
    }

    #[test]
    fn canvas_stops_at_capacity() {
        let page = page(128, 900);
        let mut stitcher = Stitcher::new(128);
        stitcher.max_rows = 250;
        stitcher.push(&frame(&page, 0, 240)).unwrap();
        assert_eq!(
            stitcher.push(&frame(&page, 100, 240)).unwrap().added_rows,
            10
        );
        assert_eq!(stitcher.rows(), 250);
        assert!(stitcher.is_full());
    }
}
