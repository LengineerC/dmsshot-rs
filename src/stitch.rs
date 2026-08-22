use crate::{
    error::{Error, Result},
    imagebuf::ImageBuffer,
};

const MAX_CANVAS_BYTES: usize = 256 << 20;
const MAX_ROWS: usize = 30_000;

// Keep these values in sync with DMS core/internal/screenshot/stitch.go.
const ACCEPT_DIFF: f32 = 9.0;
const APPROX_DIFF: f32 = 1.0;
const MIN_COMPARE: usize = 50;
const MIN_CANVAS: usize = 100;
const MIN_APPEND: usize = 15;
const COARSE_STEP: i32 = 8;
const PREDICT_WINDOW: i32 = 160;
const BAND_SAMPLES: usize = 17;

const DUPLICATE_AVERAGE_DIFF: f32 = 1.1;
const DUPLICATE_MAX_DIFF: f32 = 4.0;
const SIGNATURE_COLUMNS: usize = 18;
const SIGNATURE_ROWS: usize = 24;

const ACTIVITY_MIN: f32 = 2.0;
const ROW_MATCH_TOLERANCE: f32 = 4.0;
const MIN_ACTIVE_ROWS: usize = 12;

type RowColumns = [f32; 3];

pub struct FrameAnalysis {
    columns: Vec<RowColumns>,
    signature: Vec<f32>,
}

impl FrameAnalysis {
    pub fn signature(&self) -> &[f32] {
        &self.signature
    }
}

#[derive(Debug)]
pub struct Stitcher {
    width: u32,
    sample_x: [Vec<usize>; 3],
    canvas: Vec<u8>,
    columns: Vec<RowColumns>,
    anchor: i32,
    last: Vec<RowColumns>,
    last_offset: i32,
    max_rows: usize,
    full: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushResult {
    pub added_rows: usize,
    pub placed: bool,
}

impl Stitcher {
    pub fn new(width: u32) -> Self {
        let stride = (width as usize).saturating_mul(4).max(1);
        let pixels = width as usize;
        let bands = [(0.08, 0.32), (0.34, 0.66), (0.68, 0.92)];
        let mut sample_x: [Vec<usize>; 3] = std::array::from_fn(|_| Vec::new());
        for (index, (lower, upper)) in bands.into_iter().enumerate() {
            let start = (pixels as f64 * lower) as usize;
            let end = ((pixels as f64 * upper) as usize).max(start + 1);
            let count = BAND_SAMPLES.min(end - start);
            for sample in 0..count {
                sample_x[index].push(start + (end - start) * sample / count);
            }
        }
        Self {
            width,
            sample_x,
            canvas: Vec::new(),
            columns: Vec::new(),
            anchor: 0,
            last: Vec::new(),
            last_offset: 0,
            max_rows: (MAX_CANVAS_BYTES / stride).min(MAX_ROWS),
            full: false,
        }
    }

    pub fn rows(&self) -> usize {
        self.columns.len()
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    pub fn analyze(&self, frame: &ImageBuffer) -> Result<FrameAnalysis> {
        self.validate(frame)?;
        Ok(FrameAnalysis {
            columns: self.row_samples(&frame.pixels),
            signature: self.frame_signature(&frame.pixels, frame.height as usize),
        })
    }

    #[cfg(test)]
    pub fn push(&mut self, frame: &ImageBuffer) -> Result<PushResult> {
        let analysis = self.analyze(frame)?;
        self.push_analyzed(frame, &analysis)
    }

    pub fn push_analyzed(
        &mut self,
        frame: &ImageBuffer,
        analysis: &FrameAnalysis,
    ) -> Result<PushResult> {
        self.validate_analysis(frame, analysis)?;
        if self.full || analysis.columns.is_empty() {
            return Ok(PushResult {
                added_rows: 0,
                placed: true,
            });
        }

        let frame_height = analysis.columns.len();
        if self.columns.is_empty() {
            let added = self.append_rows(&frame.pixels, &analysis.columns, 0);
            self.anchor = 0;
            self.last.clone_from(&analysis.columns);
            self.last_offset = 0;
            return Ok(PushResult {
                added_rows: added,
                placed: true,
            });
        }

        let active = activity(&analysis.columns);
        let Some(mut position) = self.locate_frame(&analysis.columns, &active) else {
            return Ok(PushResult {
                added_rows: 0,
                placed: false,
            });
        };

        let delta = position - self.anchor;
        let mut added = 0;
        let bottom_overhang = position + frame_height as i32 - self.columns.len() as i32;
        if bottom_overhang >= MIN_APPEND as i32 {
            added += self.append_rows(
                &frame.pixels,
                &analysis.columns,
                frame_height - bottom_overhang as usize,
            );
        }
        let top_overhang = -position;
        if top_overhang >= MIN_APPEND as i32 {
            let prepended =
                self.prepend_rows(&frame.pixels, &analysis.columns, top_overhang as usize);
            added += prepended;
            position += prepended as i32;
        }

        self.anchor = position;
        self.last.clone_from(&analysis.columns);
        self.last_offset = delta;
        Ok(PushResult {
            added_rows: added,
            placed: true,
        })
    }

    #[cfg(test)]
    pub fn seam_append(&mut self, frame: &ImageBuffer) -> Result<usize> {
        let analysis = self.analyze(frame)?;
        self.seam_append_analyzed(frame, &analysis)
    }

    pub fn seam_append_analyzed(
        &mut self,
        frame: &ImageBuffer,
        analysis: &FrameAnalysis,
    ) -> Result<usize> {
        self.validate_analysis(frame, analysis)?;
        if self.full || analysis.columns.is_empty() {
            return Ok(0);
        }
        let position = self.columns.len() as i32;
        let added = self.append_rows(&frame.pixels, &analysis.columns, 0);
        self.anchor = position;
        self.last.clone_from(&analysis.columns);
        self.last_offset = 0;
        Ok(added)
    }

    pub fn finish(self) -> Result<ImageBuffer> {
        let height = self.rows() as u32;
        let mut image = ImageBuffer::new(self.width, height)?;
        image.pixels = self.canvas;
        Ok(image)
    }

    fn row_samples(&self, data: &[u8]) -> Vec<RowColumns> {
        let rows = data.len() / self.stride();
        let mut columns = vec![[0.0; 3]; rows];
        for (y, row_columns) in columns.iter_mut().enumerate() {
            for (band, samples) in self.sample_x.iter().enumerate() {
                let mut sum = 0.0;
                for &x in samples {
                    let offset = (y * self.width as usize + x) * 4;
                    sum += luminance(&data[offset..offset + 3]);
                }
                row_columns[band] = sum / samples.len() as f32;
            }
        }
        columns
    }

    fn frame_signature(&self, data: &[u8], rows: usize) -> Vec<f32> {
        let pixels = self.width as usize;
        let mut signature = Vec::with_capacity(SIGNATURE_COLUMNS * SIGNATURE_ROWS);
        for grid_y in 0..SIGNATURE_ROWS {
            let y = (2 * grid_y + 1) * rows / (2 * SIGNATURE_ROWS);
            for grid_x in 0..SIGNATURE_COLUMNS {
                let x = (2 * grid_x + 1) * pixels / (2 * SIGNATURE_COLUMNS);
                let offset = (y * pixels + x) * 4;
                signature.push(luminance(&data[offset..offset + 3]));
            }
        }
        signature
    }

    fn locate_frame(&self, frame: &[RowColumns], active: &[bool]) -> Option<i32> {
        let (delta, difference) = self.adjacent_offset(frame, active);
        let predicted = self.anchor + delta;

        if difference <= ACCEPT_DIFF && self.verify_at(frame, active, predicted).is_some() {
            return Some(predicted);
        }
        if let Some((position, _)) = self.scan_positions(frame, active, predicted, true) {
            return Some(position);
        }
        self.scan_positions(frame, active, predicted, false)
            .map(|(position, _)| position)
    }

    fn verify_at(&self, frame: &[RowColumns], active: &[bool], position: i32) -> Option<f32> {
        let (difference, count, active_matches) = self.canvas_diff(frame, active, position);
        (count >= MIN_CANVAS && difference <= ACCEPT_DIFF && active_matches >= MIN_ACTIVE_ROWS)
            .then_some(difference)
    }

    fn adjacent_offset(&self, frame: &[RowColumns], active: &[bool]) -> (i32, f32) {
        let height = frame.len();
        if self.last.len() != height {
            return (0, f32::MAX);
        }
        let limit = height.saturating_sub(MIN_COMPARE + 1) as i32;
        let mut best_delta = 0;
        let mut best_difference = f32::MAX;
        let mut countdown = -1;

        let mut try_delta = |delta: i32| {
            if delta < -limit || delta > limit {
                return false;
            }
            let (difference, active_matches) = self.pair_diff(frame, active, delta);
            if active_matches >= MIN_ACTIVE_ROWS && difference < best_difference {
                best_difference = difference;
                best_delta = delta;
            }
            if best_difference < APPROX_DIFF / 4.0 {
                return true;
            }
            if best_difference < APPROX_DIFF && countdown < 0 {
                countdown = 10;
            }
            if countdown > 0 {
                countdown -= 1;
            }
            countdown == 0
        };

        if try_delta(self.last_offset) {
            return (best_delta, best_difference);
        }
        for distance in 1.. {
            let low = self.last_offset - distance;
            let high = self.last_offset + distance;
            if low < -limit && high > limit {
                break;
            }
            if try_delta(high) || try_delta(low) {
                break;
            }
        }
        (best_delta, best_difference)
    }

    fn pair_diff(&self, frame: &[RowColumns], active: &[bool], delta: i32) -> (f32, usize) {
        let height = frame.len() as i32;
        let (ignore_top, ignore_bottom) = match_ignores(frame.len());
        let start = (ignore_top as i32).max(-delta);
        let end = (height - ignore_bottom as i32).min(height - delta);
        let count = end - start;
        if count < MIN_COMPARE as i32 {
            return (f32::MAX, 0);
        }
        let mut sum = 0.0;
        let mut active_matches = 0;
        for row in start..end {
            let difference =
                row_columns_diff(frame[row as usize], self.last[(row + delta) as usize]);
            sum += difference;
            if active[row as usize] && difference <= ROW_MATCH_TOLERANCE {
                active_matches += 1;
            }
        }
        (sum / count as f32, active_matches)
    }

    fn canvas_diff(
        &self,
        frame: &[RowColumns],
        active: &[bool],
        position: i32,
    ) -> (f32, usize, usize) {
        let height = frame.len() as i32;
        let (ignore_top, ignore_bottom) = match_ignores(frame.len());
        let start = (ignore_top as i32).max(-position);
        let end = (height - ignore_bottom as i32).min(self.columns.len() as i32 - position);
        let count = end - start;
        if count < 1 {
            return (f32::MAX, 0, 0);
        }
        let mut sum = 0.0;
        let mut active_matches = 0;
        for row in start..end {
            let difference =
                row_columns_diff(frame[row as usize], self.columns[(position + row) as usize]);
            sum += difference;
            if active[row as usize] && difference <= ROW_MATCH_TOLERANCE {
                active_matches += 1;
            }
        }
        (sum / count as f32, count as usize, active_matches)
    }

    fn scan_positions(
        &self,
        frame: &[RowColumns],
        active: &[bool],
        predicted: i32,
        near_only: bool,
    ) -> Option<(i32, f32)> {
        let height = frame.len() as i32;
        let canvas_rows = self.columns.len() as i32;
        let minimum = MIN_CANVAS as i32 - height;
        let maximum = canvas_rows - MIN_CANVAS as i32;
        if minimum > maximum {
            return None;
        }

        let mut best_position = 0;
        let mut best_difference = f32::MAX;
        let mut best_distance = i32::MAX;
        let mut consider = |position: i32| {
            if position < minimum || position > maximum {
                return;
            }
            let Some(difference) = self.verify_at(frame, active, position) else {
                return;
            };
            let distance = (position - predicted).abs();
            let better = if near_only {
                difference < best_difference
            } else {
                distance < best_distance
                    || (distance == best_distance && difference < best_difference)
            };
            if better {
                best_position = position;
                best_difference = difference;
                best_distance = distance;
            }
        };

        if near_only {
            for position in predicted - PREDICT_WINDOW..=predicted + PREDICT_WINDOW {
                consider(position);
            }
            for position in canvas_rows - height..=maximum {
                consider(position);
            }
            for position in minimum..=0 {
                consider(position);
            }
            return (best_difference <= ACCEPT_DIFF).then_some((best_position, best_difference));
        }

        let mut position = minimum;
        while position <= maximum {
            consider(position);
            position += COARSE_STEP;
        }
        if best_difference > ACCEPT_DIFF {
            return None;
        }

        let mut refined_position = best_position;
        let mut refined_difference = best_difference;
        for position in best_position - COARSE_STEP + 1..best_position + COARSE_STEP {
            if position == best_position {
                continue;
            }
            if let Some(difference) = self.verify_at(frame, active, position)
                && difference < refined_difference
            {
                refined_position = position;
                refined_difference = difference;
            }
        }
        Some((refined_position, refined_difference))
    }

    fn append_rows(&mut self, pixels: &[u8], columns: &[RowColumns], start_row: usize) -> usize {
        let requested = columns.len() - start_row;
        let available = self.max_rows.saturating_sub(self.columns.len());
        let count = requested.min(available);
        if count < requested {
            self.full = true;
        }
        if count == 0 {
            self.full = true;
            return 0;
        }
        let stride = self.stride();
        self.canvas
            .extend_from_slice(&pixels[start_row * stride..(start_row + count) * stride]);
        self.columns
            .extend_from_slice(&columns[start_row..start_row + count]);
        count
    }

    fn prepend_rows(&mut self, pixels: &[u8], columns: &[RowColumns], requested: usize) -> usize {
        let available = self.max_rows.saturating_sub(self.columns.len());
        let count = requested.min(available);
        if count < requested {
            self.full = true;
        }
        if count == 0 {
            self.full = true;
            return 0;
        }

        let stride = self.stride();
        let mut canvas = Vec::with_capacity(count * stride + self.canvas.len());
        canvas.extend_from_slice(&pixels[..count * stride]);
        canvas.append(&mut self.canvas);
        self.canvas = canvas;

        let mut next_columns = Vec::with_capacity(count + self.columns.len());
        next_columns.extend_from_slice(&columns[..count]);
        next_columns.append(&mut self.columns);
        self.columns = next_columns;
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

    fn validate_analysis(&self, frame: &ImageBuffer, analysis: &FrameAnalysis) -> Result<()> {
        self.validate(frame)?;
        if analysis.columns.len() != frame.height as usize {
            return Err(Error::InvalidBuffer(
                "scroll frame analysis has the wrong height".into(),
            ));
        }
        Ok(())
    }

    fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

pub fn duplicate_frame(first: &[f32], second: &[f32]) -> bool {
    if first.len() != second.len() || first.is_empty() {
        return false;
    }
    let mut sum = 0.0;
    let mut maximum: f32 = 0.0;
    for (&a, &b) in first.iter().zip(second) {
        let difference = (a - b).abs();
        sum += difference;
        maximum = maximum.max(difference);
    }
    sum / first.len() as f32 <= DUPLICATE_AVERAGE_DIFF && maximum <= DUPLICATE_MAX_DIFF
}

fn row_columns_diff(first: RowColumns, second: RowColumns) -> f32 {
    ((first[0] - second[0]).abs() + (first[1] - second[1]).abs() + (first[2] - second[2]).abs())
        / 3.0
}

fn match_ignores(height: usize) -> (usize, usize) {
    if height < 80 {
        return (0, 0);
    }
    (
        (height / 10).clamp(16, height / 4),
        (height * 8 / 100).clamp(16, height / 4),
    )
}

fn activity(frame: &[RowColumns]) -> Vec<bool> {
    let mut active = vec![false; frame.len()];
    for row in 1..frame.len() {
        active[row] = row_columns_diff(frame[row], frame[row - 1]) > ACTIVITY_MIN;
    }
    active
}

fn luminance(pixel: &[u8]) -> f32 {
    0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_WIDTH: u32 = 128;
    const TEST_FRAME_HEIGHT: u32 = 240;

    #[derive(Clone)]
    struct TestRng(u32);

    impl TestRng {
        fn new(seed: u32) -> Self {
            Self(seed)
        }

        fn byte(&mut self) -> u8 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 24) as u8
        }

        fn range(&mut self, upper: usize) -> usize {
            usize::from(self.byte()) % upper
        }

        fn fill(&mut self, bytes: &mut [u8]) {
            for byte in bytes {
                *byte = self.byte();
            }
        }
    }

    fn page(width: u32, height: u32) -> ImageBuffer {
        let mut image = ImageBuffer::new(width, height).unwrap();
        let mut random = TestRng::new(0x1234_5678);
        random.fill(&mut image.pixels);
        for alpha in image.pixels.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        image
    }

    fn frame(page: &ImageBuffer, top: i32, height: i32) -> ImageBuffer {
        page.crop(0, top, page.width as i32, height).unwrap()
    }

    #[test]
    fn stitches_sliding_windows_exactly() {
        let page = page(TEST_WIDTH, 1_000);
        for delta in [20, 60, 110] {
            let mut stitcher = Stitcher::new(TEST_WIDTH);
            let mut last_top = 0;
            let mut top = 0;
            while top + TEST_FRAME_HEIGHT <= 900 {
                last_top = top;
                assert!(
                    stitcher
                        .push(&frame(&page, top as i32, TEST_FRAME_HEIGHT as i32))
                        .unwrap()
                        .placed
                );
                top += delta;
            }
            let expected_rows = last_top + TEST_FRAME_HEIGHT;
            let image = stitcher.finish().unwrap();
            assert_eq!(image.height, expected_rows);
            assert_eq!(
                image.pixels,
                page.pixels[..expected_rows as usize * TEST_WIDTH as usize * 4]
            );
        }
    }

    #[test]
    fn drops_frames_without_overlap() {
        let page = page(TEST_WIDTH, 1_000);
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        stitcher
            .push(&frame(&page, 0, TEST_FRAME_HEIGHT as i32))
            .unwrap();
        let result = stitcher
            .push(&frame(
                &page,
                TEST_FRAME_HEIGHT as i32 + 50,
                TEST_FRAME_HEIGHT as i32,
            ))
            .unwrap();
        assert!(!result.placed);
        assert_eq!(stitcher.rows(), TEST_FRAME_HEIGHT as usize);
    }

    #[test]
    fn revisits_and_small_jitter_do_not_grow_canvas() {
        let page = page(TEST_WIDTH, 1_000);
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        for top in [0, 100, 200, 150, 60, 0, 80, 190, 300] {
            stitcher
                .push(&frame(&page, top, TEST_FRAME_HEIGHT as i32))
                .unwrap();
        }
        assert_eq!(stitcher.rows(), 300 + TEST_FRAME_HEIGHT as usize);
        assert_eq!(
            stitcher.canvas,
            page.pixels[..stitcher.rows() * TEST_WIDTH as usize * 4]
        );

        let mut jitter = Stitcher::new(TEST_WIDTH);
        jitter
            .push(&frame(&page, 0, TEST_FRAME_HEIGHT as i32))
            .unwrap();
        assert_eq!(
            jitter
                .push(&frame(
                    &page,
                    MIN_APPEND as i32 - 5,
                    TEST_FRAME_HEIGHT as i32
                ))
                .unwrap()
                .added_rows,
            0
        );
    }

    #[test]
    fn scrolling_up_prepends_without_corruption() {
        let page = page(TEST_WIDTH, 1_000);
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        stitcher
            .push(&frame(&page, 500, TEST_FRAME_HEIGHT as i32))
            .unwrap();
        assert_eq!(
            stitcher
                .push(&frame(&page, 420, TEST_FRAME_HEIGHT as i32))
                .unwrap()
                .added_rows,
            80
        );
        stitcher
            .push(&frame(&page, 560, TEST_FRAME_HEIGHT as i32))
            .unwrap();
        assert_eq!(
            stitcher.canvas,
            page.pixels[420 * TEST_WIDTH as usize * 4
                ..(560 + TEST_FRAME_HEIGHT as usize) * TEST_WIDTH as usize * 4]
        );
    }

    #[test]
    fn fixed_outer_chrome_and_hover_noise_are_ignored() {
        let page = page(TEST_WIDTH, 1_000);
        let decorate = |top: i32, seed: u8| {
            let mut image = frame(&page, top, TEST_FRAME_HEIGHT as i32);
            for y in 0..TEST_FRAME_HEIGHT as usize {
                for x in 0..8 {
                    let index = (y * TEST_WIDTH as usize + x) * 4;
                    image.pixels[index] = seed.wrapping_add(y as u8);
                }
            }
            for y in 100..124 {
                for x in 48..58 {
                    let index = (y * TEST_WIDTH as usize + x) * 4;
                    image.pixels[index] ^= 8;
                }
            }
            image
        };
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        stitcher.push(&decorate(0, 1)).unwrap();
        assert_eq!(stitcher.push(&decorate(90, 2)).unwrap().added_rows, 90);
    }

    #[test]
    fn blank_frames_do_not_create_false_growth() {
        let blank = ImageBuffer::new(TEST_WIDTH, TEST_FRAME_HEIGHT).unwrap();
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        stitcher.push(&blank).unwrap();
        let result = stitcher.push(&blank).unwrap();
        assert_eq!(result.added_rows, 0);
        assert_eq!(stitcher.rows(), TEST_FRAME_HEIGHT as usize);
    }

    #[test]
    fn seam_recovers_after_fast_jump() {
        let page = page(TEST_WIDTH, 1_200);
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        stitcher
            .push(&frame(&page, 0, TEST_FRAME_HEIGHT as i32))
            .unwrap();
        let jump = frame(&page, 700, TEST_FRAME_HEIGHT as i32);
        assert!(!stitcher.push(&jump).unwrap().placed);
        assert_eq!(stitcher.seam_append(&jump).unwrap(), 240);
        assert_eq!(
            stitcher
                .push(&frame(&page, 760, TEST_FRAME_HEIGHT as i32))
                .unwrap()
                .added_rows,
            60
        );
    }

    #[test]
    fn canvas_stops_at_capacity() {
        let page = page(TEST_WIDTH, 900);
        let mut stitcher = Stitcher::new(TEST_WIDTH);
        stitcher.max_rows = TEST_FRAME_HEIGHT as usize + 10;
        stitcher
            .push(&frame(&page, 0, TEST_FRAME_HEIGHT as i32))
            .unwrap();
        assert_eq!(
            stitcher
                .push(&frame(&page, 100, TEST_FRAME_HEIGHT as i32))
                .unwrap()
                .added_rows,
            10
        );
        assert!(stitcher.is_full());
    }

    struct SimSession {
        previous_signature: Vec<f32>,
        previous_placed: bool,
        unmatched: bool,
        unmatched_ticks: u8,
        stitcher: Stitcher,
    }

    impl SimSession {
        fn new(width: u32) -> Self {
            Self {
                previous_signature: Vec::new(),
                previous_placed: false,
                unmatched: false,
                unmatched_ticks: 0,
                stitcher: Stitcher::new(width),
            }
        }

        fn observe(&mut self, frame: &ImageBuffer) {
            let analysis = self.stitcher.analyze(frame).unwrap();
            let signature = analysis.signature().to_vec();
            let duplicate = duplicate_frame(&signature, &self.previous_signature);
            self.previous_signature = signature;

            if duplicate && self.unmatched {
                self.unmatched_ticks += 1;
                if self.unmatched_ticks < 4 {
                    return;
                }
                let result = self.stitcher.push_analyzed(frame, &analysis).unwrap();
                if !result.placed {
                    self.stitcher
                        .seam_append_analyzed(frame, &analysis)
                        .unwrap();
                }
                self.previous_placed = true;
                self.unmatched = false;
                self.unmatched_ticks = 0;
            } else if !(duplicate && self.previous_placed) {
                let result = self.stitcher.push_analyzed(frame, &analysis).unwrap();
                self.previous_placed = result.placed;
                self.unmatched = !result.placed;
                self.unmatched_ticks = 0;
            }
        }
    }

    fn webby_page(random: &mut TestRng, width: u32, rows: u32) -> ImageBuffer {
        let mut page = ImageBuffer::new(width, rows).unwrap();
        let stride = width as usize * 4;
        let mut card = vec![0; 40 * stride];
        random.fill(&mut card);
        let mut row = 0usize;
        while row < rows as usize {
            match random.range(4) {
                0 => row = (row + 10 + random.range(20)).min(rows as usize),
                1 => {
                    let count = (rows as usize - row).min(40);
                    page.pixels[row * stride..(row + count) * stride]
                        .copy_from_slice(&card[..count * stride]);
                    row += count;
                }
                _ => {
                    let count = (8 + random.range(22)).min(rows as usize - row);
                    random.fill(&mut page.pixels[row * stride..(row + count) * stride]);
                    row += count;
                }
            }
        }
        page
    }

    fn fractional_frame(page: &ImageBuffer, offset: f64, frame_height: u32) -> ImageBuffer {
        let top = offset as usize;
        let fraction = offset - top as f64;
        let mut frame = ImageBuffer::new(page.width, frame_height).unwrap();
        let stride = page.width as usize * 4;
        for row in 0..frame_height as usize {
            let first = &page.pixels[(top + row) * stride..(top + row + 1) * stride];
            let second = &page.pixels[(top + row + 1) * stride..(top + row + 2) * stride];
            let destination = &mut frame.pixels[row * stride..(row + 1) * stride];
            for ((destination, &first), &second) in destination.iter_mut().zip(first).zip(second) {
                *destination = (first as f64 * (1.0 - fraction) + second as f64 * fraction) as u8;
            }
        }
        frame
    }

    fn add_fixed_chrome(
        random: &mut TestRng,
        frame: &mut ImageBuffer,
        sidebar: &[u8],
        sidebar_width: usize,
    ) {
        let stride = frame.width as usize * 4;
        let sidebar_stride = sidebar_width * 4;
        for row in 0..frame.height as usize {
            frame.pixels[row * stride..row * stride + sidebar_stride]
                .copy_from_slice(&sidebar[row * sidebar_stride..(row + 1) * sidebar_stride]);
        }
        let hover_top = 40 + random.range(frame.height as usize - 80);
        let hover_x = frame.width as usize / 3;
        for row in hover_top..hover_top + 24 {
            let start = (row * frame.width as usize + hover_x) * 4;
            for byte in &mut frame.pixels[start..start + 15 * 4] {
                *byte ^= 8;
            }
        }
    }

    fn capture_web_frame(
        page: &ImageBuffer,
        random: &mut TestRng,
        sidebar: &[u8],
        sidebar_width: usize,
        position: f64,
    ) -> ImageBuffer {
        let mut frame = fractional_frame(page, position, TEST_FRAME_HEIGHT);
        add_fixed_chrome(random, &mut frame, sidebar, sidebar_width);
        frame
    }

    fn glide(
        session: &mut SimSession,
        page: &ImageBuffer,
        random: &mut TestRng,
        sidebar: &[u8],
        sidebar_width: usize,
        position: &mut f64,
        target: f64,
    ) {
        for iteration in 0.. {
            let step = (target - *position) * 0.45;
            if step.abs() < 1.0 {
                break;
            }
            *position += step;
            if iteration % 4 != 3 {
                *position = position.trunc();
            }
            session.observe(&capture_web_frame(
                page,
                random,
                sidebar,
                sidebar_width,
                *position,
            ));
        }
        *position = target;
        let frame = capture_web_frame(page, random, sidebar, sidebar_width, *position);
        session.observe(&frame);
        session.observe(&frame);
    }

    #[test]
    fn smooth_web_scroll_with_fixed_sidebar_does_not_scramble_rows() {
        const WIDTH: u32 = 256;
        const SIDEBAR_WIDTH: usize = 16;
        let mut random = TestRng::new(99);
        let page = webby_page(&mut random, WIDTH, 4_000);
        let mut sidebar = vec![0; TEST_FRAME_HEIGHT as usize * SIDEBAR_WIDTH * 4];
        random.fill(&mut sidebar);
        let mut session = SimSession::new(WIDTH);
        let mut position = 0.0;

        session.observe(&capture_web_frame(
            &page,
            &mut random,
            &sidebar,
            SIDEBAR_WIDTH,
            position,
        ));
        for target in [160.0, 330.0, 480.0, 650.0, 800.0, 960.0, 1_100.0] {
            glide(
                &mut session,
                &page,
                &mut random,
                &sidebar,
                SIDEBAR_WIDTH,
                &mut position,
                target,
            );
        }
        for target in [700.0, 300.0, 900.0, 1_100.0] {
            glide(
                &mut session,
                &page,
                &mut random,
                &sidebar,
                SIDEBAR_WIDTH,
                &mut position,
                target,
            );
        }

        let expected_rows = 1_100 + TEST_FRAME_HEIGHT as usize;
        let actual_rows = session.stitcher.rows();
        assert!(
            (expected_rows - MIN_APPEND..=expected_rows + 2).contains(&actual_rows),
            "stitched {actual_rows} rows, expected approximately {expected_rows}"
        );

        let stride = WIDTH as usize * 4;
        let hover_start = WIDTH as usize / 3 * 4;
        let hover_end = hover_start + 15 * 4;
        let mut mismatched = 0;
        for row in (0..actual_rows.min(expected_rows)).step_by(7) {
            let offset = row * stride;
            let canvas = &session.stitcher.canvas[offset..offset + stride];
            let source = &page.pixels[offset..offset + stride];
            if canvas[SIDEBAR_WIDTH * 4..hover_start] != source[SIDEBAR_WIDTH * 4..hover_start]
                || canvas[hover_end..] != source[hover_end..]
            {
                mismatched += 1;
            }
        }
        assert!(
            mismatched <= (expected_rows / 7) / 20,
            "{mismatched} sampled rows contain misplaced or animated content"
        );
    }

    #[test]
    fn smooth_bottom_up_scroll_prepends_in_page_order() {
        const WIDTH: u32 = 256;
        const SIDEBAR_WIDTH: usize = 16;
        let mut random = TestRng::new(99);
        let page = webby_page(&mut random, WIDTH, 4_000);
        let mut sidebar = vec![0; TEST_FRAME_HEIGHT as usize * SIDEBAR_WIDTH * 4];
        random.fill(&mut sidebar);
        let mut session = SimSession::new(WIDTH);
        let mut position = 3_700.0;

        session.observe(&capture_web_frame(
            &page,
            &mut random,
            &sidebar,
            SIDEBAR_WIDTH,
            position,
        ));
        for target in [3_640.0, 3_560.0, 3_460.0, 3_340.0, 3_240.0] {
            glide(
                &mut session,
                &page,
                &mut random,
                &sidebar,
                SIDEBAR_WIDTH,
                &mut position,
                target,
            );
        }

        let expected_rows = 3_700 + TEST_FRAME_HEIGHT as usize - 3_240;
        let actual_rows = session.stitcher.rows();
        assert!(
            (expected_rows - MIN_APPEND..=expected_rows + 2).contains(&actual_rows),
            "stitched {actual_rows} rows, expected approximately {expected_rows}"
        );
        let source_top = 3_240 + expected_rows - actual_rows;
        for canvas_row in [0, 100, 300] {
            assert!(row_matches_page(
                &session.stitcher.canvas,
                &page.pixels,
                WIDTH as usize * 4,
                canvas_row,
                source_top + canvas_row,
            ));
        }
    }

    fn row_matches_page(
        canvas: &[u8],
        page: &[u8],
        stride: usize,
        canvas_row: usize,
        page_row: usize,
    ) -> bool {
        for byte in 32 * 4..70 * 4 {
            let captured = canvas[canvas_row * stride + byte];
            let mut lower = u8::MAX;
            let mut upper = u8::MIN;
            for candidate in page_row - 1..=page_row + 1 {
                let value = page[candidate * stride + byte];
                lower = lower.min(value);
                upper = upper.max(value);
            }
            if captured < lower.saturating_sub(1) || captured > upper.saturating_add(1) {
                return false;
            }
        }
        true
    }

    #[test]
    fn fast_fling_waits_for_stable_frame_before_starting_a_segment() {
        const WIDTH: u32 = 256;
        let mut random = TestRng::new(7);
        let page = webby_page(&mut random, WIDTH, 4_000);
        let mut session = SimSession::new(WIDTH);
        let stable = |top: i32| frame(&page, top, TEST_FRAME_HEIGHT as i32);
        let rest = |session: &mut SimSession, top: i32| {
            let frame = stable(top);
            for _ in 0..6 {
                session.observe(&frame);
            }
        };

        rest(&mut session, 0);
        session.observe(&stable(60));
        session.observe(&stable(130));
        rest(&mut session, 130);
        let first_range = 130 + TEST_FRAME_HEIGHT as usize;

        session.observe(&stable(900));
        session.observe(&stable(1_400));
        rest(&mut session, 1_800);
        session.observe(&stable(1_860));
        session.observe(&stable(1_930));
        rest(&mut session, 1_930);

        let expected_rows = first_range + 130 + TEST_FRAME_HEIGHT as usize;
        assert_eq!(session.stitcher.rows(), expected_rows);
        let stride = WIDTH as usize * 4;
        assert_eq!(
            session.stitcher.canvas[first_range * stride..],
            page.pixels[1_800 * stride..(1_930 + TEST_FRAME_HEIGHT as usize) * stride]
        );
    }
}
