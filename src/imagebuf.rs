use std::io::Write;

use image::{
    ExtendedColorType, ImageEncoder,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
};

use crate::{
    cli::OutputFormat,
    error::{Error, Result},
};

pub const WL_SHM_ARGB8888: u32 = 0;
pub const WL_SHM_XRGB8888: u32 = 1;
pub const WL_SHM_ABGR8888: u32 = 0x3432_4241;
pub const WL_SHM_XBGR8888: u32 = 0x3432_4258;
pub const WL_SHM_RGB888: u32 = 0x3432_4752;
pub const WL_SHM_BGR888: u32 = 0x3432_4742;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

impl Region {
    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub scale: f64,
    pub region: Region,
}

impl ImageBuffer {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let len = byte_len(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
            scale: 1.0,
            region: Region {
                width: width as i32,
                height: height as i32,
                ..Region::default()
            },
        })
    }

    pub fn from_wayland(
        width: u32,
        height: u32,
        stride: u32,
        format: u32,
        source: &[u8],
        y_inverted: bool,
    ) -> Result<Self> {
        let bytes_per_pixel = match format {
            WL_SHM_RGB888 | WL_SHM_BGR888 => 3,
            WL_SHM_ARGB8888 | WL_SHM_XRGB8888 | WL_SHM_ABGR8888 | WL_SHM_XBGR8888 => 4,
            other => {
                return Err(Error::InvalidBuffer(format!(
                    "unsupported wl_shm format 0x{other:08x}"
                )));
            }
        };
        let min_stride = width
            .checked_mul(bytes_per_pixel)
            .ok_or(Error::ImageTooLarge)?;
        if stride < min_stride {
            return Err(Error::InvalidBuffer(format!(
                "stride {stride} is smaller than {width}x{bytes_per_pixel}"
            )));
        }
        let source_len = (stride as usize)
            .checked_mul(height as usize)
            .ok_or(Error::ImageTooLarge)?;
        if source.len() < source_len {
            return Err(Error::InvalidBuffer(format!(
                "buffer has {} bytes, expected at least {source_len}",
                source.len()
            )));
        }

        let mut image = Self::new(width, height)?;
        for dst_y in 0..height as usize {
            let src_y = if y_inverted {
                height as usize - 1 - dst_y
            } else {
                dst_y
            };
            let src_row = src_y * stride as usize;
            let dst_row = dst_y * width as usize * 4;
            for x in 0..width as usize {
                let si = src_row + x * bytes_per_pixel as usize;
                let di = dst_row + x * 4;
                let (r, g, b, alpha) = match format {
                    WL_SHM_ABGR8888 => (source[si], source[si + 1], source[si + 2], source[si + 3]),
                    WL_SHM_XBGR8888 => (source[si], source[si + 1], source[si + 2], 255),
                    WL_SHM_RGB888 | WL_SHM_XRGB8888 => {
                        (source[si + 2], source[si + 1], source[si], 255)
                    }
                    WL_SHM_ARGB8888 => (source[si + 2], source[si + 1], source[si], source[si + 3]),
                    WL_SHM_BGR888 => (source[si], source[si + 1], source[si + 2], 255),
                    _ => unreachable!(),
                };
                let (r, g, b) = if alpha == 0 || alpha == 255 {
                    (r, g, b)
                } else {
                    (
                        unpremultiply(r, alpha),
                        unpremultiply(g, alpha),
                        unpremultiply(b, alpha),
                    )
                };
                image.pixels[di..di + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        Ok(image)
    }

    pub fn crop(&self, x: i32, y: i32, width: i32, height: i32) -> Result<Self> {
        if width <= 0 || height <= 0 {
            return Err(Error::InvalidBuffer("empty crop".into()));
        }
        let x0 = x.clamp(0, self.width as i32);
        let y0 = y.clamp(0, self.height as i32);
        let x1 = x.saturating_add(width).clamp(0, self.width as i32);
        let y1 = y.saturating_add(height).clamp(0, self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return Err(Error::InvalidBuffer("crop is outside the image".into()));
        }
        let mut out = Self::new((x1 - x0) as u32, (y1 - y0) as u32)?;
        for row in 0..out.height as usize {
            let si = ((y0 as usize + row) * self.width as usize + x0 as usize) * 4;
            let di = row * out.width as usize * 4;
            let len = out.width as usize * 4;
            out.pixels[di..di + len].copy_from_slice(&self.pixels[si..si + len]);
        }
        out.scale = self.scale;
        out.region = Region {
            x: self.region.x + x0,
            y: self.region.y + y0,
            width: out.width as i32,
            height: out.height as i32,
            output: self.region.output.clone(),
        };
        Ok(out)
    }

    pub fn apply_inverse_transform(&self, transform: i32) -> Result<Self> {
        let inverse = match transform {
            1 => 3,
            3 => 1,
            5 => 7,
            7 => 5,
            value => value,
        };
        self.apply_transform(inverse)
    }

    pub fn apply_transform(&self, transform: i32) -> Result<Self> {
        if transform == 0 {
            return Ok(self.clone());
        }
        let swaps_axes = matches!(transform, 1 | 3 | 5 | 7);
        let (new_width, new_height) = if swaps_axes {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let mut out = Self::new(new_width, new_height)?;
        for sy in 0..self.height as i32 {
            for sx in 0..self.width as i32 {
                let (dx, dy) = match transform {
                    1 => (sy, self.width as i32 - 1 - sx),
                    2 => (self.width as i32 - 1 - sx, self.height as i32 - 1 - sy),
                    3 => (self.height as i32 - 1 - sy, sx),
                    4 => (self.width as i32 - 1 - sx, sy),
                    5 => (sy, sx),
                    6 => (sx, self.height as i32 - 1 - sy),
                    7 => (self.height as i32 - 1 - sy, self.width as i32 - 1 - sx),
                    _ => {
                        return Err(Error::InvalidBuffer(format!(
                            "invalid output transform {transform}"
                        )));
                    }
                };
                let si = (sy as usize * self.width as usize + sx as usize) * 4;
                let di = (dy as usize * out.width as usize + dx as usize) * 4;
                out.pixels[di..di + 4].copy_from_slice(&self.pixels[si..si + 4]);
            }
        }
        out.scale = self.scale;
        out.region = self.region.clone();
        out.region.width = out.width as i32;
        out.region.height = out.height as i32;
        Ok(out)
    }

    pub fn blit_nearest(&mut self, source: &Self, x: i32, y: i32, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        for dy in 0..height {
            let ty = y + dy as i32;
            if !(0..self.height as i32).contains(&ty) {
                continue;
            }
            let sy = dy as usize * source.height as usize / height as usize;
            for dx in 0..width {
                let tx = x + dx as i32;
                if !(0..self.width as i32).contains(&tx) {
                    continue;
                }
                let sx = dx as usize * source.width as usize / width as usize;
                let si = (sy * source.width as usize + sx) * 4;
                let di = (ty as usize * self.width as usize + tx as usize) * 4;
                self.pixels[di..di + 4].copy_from_slice(&source.pixels[si..si + 4]);
            }
        }
    }

    pub fn write_encoded<W: Write>(
        &self,
        mut writer: W,
        format: OutputFormat,
        quality: u8,
    ) -> Result<()> {
        match format {
            OutputFormat::Png => PngEncoder::new(&mut writer).write_image(
                &self.pixels,
                self.width,
                self.height,
                ExtendedColorType::Rgba8,
            )?,
            OutputFormat::Jpg => {
                let mut rgb = Vec::with_capacity(self.width as usize * self.height as usize * 3);
                for pixel in self.pixels.chunks_exact(4) {
                    rgb.extend_from_slice(&pixel[..3]);
                }
                JpegEncoder::new_with_quality(&mut writer, quality).write_image(
                    &rgb,
                    self.width,
                    self.height,
                    ExtendedColorType::Rgb8,
                )?;
            }
            OutputFormat::Ppm => {
                write!(writer, "P6\n{} {}\n255\n", self.width, self.height)?;
                for pixel in self.pixels.chunks_exact(4) {
                    writer.write_all(&pixel[..3])?;
                }
            }
        }
        Ok(())
    }
}

fn byte_len(width: u32, height: u32) -> Result<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|value| value.checked_mul(4))
        .ok_or(Error::ImageTooLarge)
}

fn unpremultiply(value: u8, alpha: u8) -> u8 {
    ((value as u16 * 255 + alpha as u16 / 2) / alpha as u16).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bgra_with_stride_and_flip() {
        let source = [
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, 7, 8, 9, 0, 10, 11, 12, 0, 88, 88, 88, 88,
        ];
        let image = ImageBuffer::from_wayland(2, 2, 12, WL_SHM_XRGB8888, &source, true).unwrap();
        assert_eq!(&image.pixels[..8], &[9, 8, 7, 255, 12, 11, 10, 255]);
        assert_eq!(&image.pixels[8..], &[3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn unpremultiplies_argb_and_abgr_before_discarding_alpha() {
        let argb = [16, 32, 64, 128];
        let image = ImageBuffer::from_wayland(1, 1, 4, WL_SHM_ARGB8888, &argb, false).unwrap();
        assert_eq!(image.pixels, vec![128, 64, 32, 255]);

        let abgr = [64, 32, 16, 128];
        let image = ImageBuffer::from_wayland(1, 1, 4, WL_SHM_ABGR8888, &abgr, false).unwrap();
        assert_eq!(image.pixels, vec![128, 64, 32, 255]);
    }

    #[test]
    fn transform_roundtrip_dimensions() {
        let mut image = ImageBuffer::new(2, 3).unwrap();
        for (index, pixel) in image.pixels.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[index as u8, 0, 0, 255]);
        }
        let rotated = image.apply_transform(1).unwrap();
        assert_eq!((rotated.width, rotated.height), (3, 2));
        assert_eq!(rotated.apply_transform(3).unwrap().pixels, image.pixels);
    }

    #[test]
    fn crop_clamps_to_bounds() {
        let image = ImageBuffer::new(10, 10).unwrap();
        let crop = image.crop(-2, 8, 6, 8).unwrap();
        assert_eq!((crop.width, crop.height), (4, 2));
    }

    #[test]
    fn all_transforms_preserve_pixels() {
        let mut image = ImageBuffer::new(3, 2).unwrap();
        for (index, pixel) in image.pixels.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[index as u8, 0, 0, 255]);
        }
        for transform in 1..=7 {
            let transformed = image.apply_transform(transform).unwrap();
            let mut values: Vec<_> = transformed
                .pixels
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect();
            values.sort_unstable();
            assert_eq!(values, vec![0, 1, 2, 3, 4, 5]);
        }
    }

    #[test]
    fn ppm_encoder_writes_rgb_only() {
        let mut image = ImageBuffer::new(1, 1).unwrap();
        image.pixels.copy_from_slice(&[1, 2, 3, 99]);
        let mut encoded = Vec::new();
        image
            .write_encoded(&mut encoded, OutputFormat::Ppm, 90)
            .unwrap();
        assert_eq!(encoded, b"P6\n1 1\n255\n\x01\x02\x03");
    }
}
