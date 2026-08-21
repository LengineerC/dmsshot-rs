use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use paddleocr_rs_onnx::{OcrEngine as PaddleEngine, OrderBy};

use crate::{
    cli::{OcrEngine, OutputFormat},
    error::{Error, Result},
    imagebuf::ImageBuffer,
};

const TARGET_OCR_SCALE: f64 = 2.0;
const MAX_UPSCALED_PIXELS: f64 = 32.0 * 1024.0 * 1024.0;
const DET_MODEL: &str = "ch_PP-OCRv4_det_infer.onnx";
const REC_MODEL: &str = "ch_PP-OCRv4_rec_infer.onnx";
const KEYS_FILE: &str = "ppocr_keys_v1.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaddleModelPaths {
    directory: PathBuf,
    detection: PathBuf,
    recognition: PathBuf,
    keys: PathBuf,
}

impl PaddleModelPaths {
    fn from_directory(directory: PathBuf) -> Self {
        Self {
            detection: directory.join(DET_MODEL),
            recognition: directory.join(REC_MODEL),
            keys: directory.join(KEYS_FILE),
            directory,
        }
    }

    fn is_complete(&self) -> bool {
        self.detection.is_file() && self.recognition.is_file() && self.keys.is_file()
    }

    fn missing_names(&self) -> Vec<&'static str> {
        [
            (!self.detection.is_file()).then_some(DET_MODEL),
            (!self.recognition.is_file()).then_some(REC_MODEL),
            (!self.keys.is_file()).then_some(KEYS_FILE),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

pub fn ensure_available(
    engine: OcrEngine,
    model_dir: Option<&Path>,
) -> Result<Option<PaddleModelPaths>> {
    match engine {
        OcrEngine::Paddle => find_paddle_models(model_dir).map(Some),
        OcrEngine::Tesseract => {
            ensure_tesseract_available()?;
            Ok(None)
        }
    }
}

pub fn recognize(
    engine: OcrEngine,
    paddle_models: Option<&PaddleModelPaths>,
    image: &ImageBuffer,
    language: &str,
    psm: Option<u8>,
) -> Result<String> {
    match engine {
        OcrEngine::Paddle => recognize_paddle(
            paddle_models.ok_or_else(|| Error::Ocr("PaddleOCR models were not resolved".into()))?,
            image,
        ),
        OcrEngine::Tesseract => recognize_tesseract(image, language, psm),
    }
}

fn find_paddle_models(explicit: Option<&Path>) -> Result<PaddleModelPaths> {
    let directories = paddle_model_directories(
        explicit,
        env::var_os("DMSSHOT_OCR_MODEL_DIR").as_deref(),
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
        env::current_exe().ok().as_deref(),
    );
    let candidates: Vec<_> = directories
        .into_iter()
        .map(PaddleModelPaths::from_directory)
        .collect();
    if let Some(models) = candidates.iter().find(|models| models.is_complete()) {
        return Ok(models.clone());
    }
    let missing = candidates
        .first()
        .map(PaddleModelPaths::missing_names)
        .unwrap_or_else(|| vec![DET_MODEL, REC_MODEL, KEYS_FILE])
        .join(", ");
    let searched = candidates
        .iter()
        .map(|models| models.directory.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error::PaddleModelsMissing { searched, missing })
}

fn paddle_model_directories(
    explicit: Option<&Path>,
    environment: Option<&std::ffi::OsStr>,
    xdg_data_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
    executable: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(directory) = explicit {
        return vec![directory.to_path_buf()];
    }
    let mut directories = Vec::new();
    if let Some(directory) = environment.filter(|value| !value.is_empty()) {
        directories.push(PathBuf::from(directory));
    }
    if let Some(directory) = xdg_data_home.filter(|value| !value.is_empty()) {
        directories.push(PathBuf::from(directory).join("dmsshot-rs/models"));
    } else if let Some(directory) = home.filter(|value| !value.is_empty()) {
        directories.push(PathBuf::from(directory).join(".local/share/dmsshot-rs/models"));
    }
    if let Some(directory) = executable
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|prefix| prefix.join("share/dmsshot-rs/models"))
    {
        directories.push(directory);
    }
    directories.push(PathBuf::from("/usr/share/dmsshot-rs/models"));
    directories.dedup();
    directories
}

fn recognize_paddle(models: &PaddleModelPaths, image: &ImageBuffer) -> Result<String> {
    let detection = read_model(&models.detection)?;
    let recognition = read_model(&models.recognition)?;
    let keys = read_model(&models.keys)?;
    let engine = PaddleEngine::new(&detection, &recognition, &keys)
        .map_err(|error| Error::Ocr(format!("unable to load PaddleOCR models: {error}")))?;

    let mut pixels = image.pixels.clone();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * alpha + 255 * (255 - alpha)) / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
    let rgba = image::RgbaImage::from_raw(image.width, image.height, pixels)
        .ok_or_else(|| Error::Ocr("unable to prepare the screenshot for PaddleOCR".into()))?;
    let blocks = engine
        .recognize_all(&image::DynamicImage::ImageRgba8(rgba), OrderBy::Horizontal)
        .map_err(|error| Error::Ocr(format!("PaddleOCR inference failed: {error}")))?
        .into_iter()
        .filter(|block| !block.text.trim().is_empty() && block.confidence >= 0.35)
        .map(|block| TextBlock {
            text: normalize_text(block.text),
            confidence: block.confidence,
            x: block.x,
            y: block.y,
            width: block.width,
            height: block.height,
        })
        .collect::<Vec<_>>();
    assemble_paddle_text(blocks)
}

fn read_model(path: &Path) -> Result<Vec<u8>> {
    fs::read(path)
        .map_err(|error| Error::Ocr(format!("unable to read {}: {error}", path.display())))
}

#[derive(Debug, Clone)]
struct TextBlock {
    text: String,
    confidence: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn assemble_paddle_text(mut blocks: Vec<TextBlock>) -> Result<String> {
    blocks.retain(|block| !block.text.trim().is_empty() && block.confidence >= 0.35);
    if blocks.is_empty() {
        return Err(Error::OcrNoText);
    }
    blocks.sort_by(|left, right| {
        left.y
            .total_cmp(&right.y)
            .then_with(|| left.x.total_cmp(&right.x))
    });

    let mut lines: Vec<Vec<TextBlock>> = Vec::new();
    for block in blocks {
        let center = block.y + block.height / 2.0;
        let line = lines.iter_mut().find(|line| {
            let first = &line[0];
            let line_center = first.y + first.height / 2.0;
            (center - line_center).abs() <= block.height.max(first.height) * 0.5
        });
        if let Some(line) = line {
            line.push(block);
        } else {
            lines.push(vec![block]);
        }
    }
    lines.sort_by(|left, right| left[0].y.total_cmp(&right[0].y));

    let text = lines
        .into_iter()
        .map(|mut line| {
            line.sort_by(|left, right| left.x.total_cmp(&right.x));
            let mut output = String::new();
            let mut previous_right = None;
            for block in line {
                let token = block.text.trim();
                let is_separate_box =
                    previous_right.is_none_or(|right| block.x >= right - block.height * 0.2);
                if !output.is_empty() && is_separate_box && needs_word_separator(&output, token) {
                    output.push(' ');
                }
                output.push_str(token);
                previous_right = Some(block.x + block.width);
            }
            output
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let text = normalize_text(text);
    if text.is_empty() {
        Err(Error::OcrNoText)
    } else {
        Ok(text)
    }
}

fn needs_word_separator(left: &str, right: &str) -> bool {
    let Some(left) = left.chars().next_back() else {
        return false;
    };
    let Some(right) = right.chars().next() else {
        return false;
    };
    !left.is_whitespace()
        && !right.is_whitespace()
        && !is_cjk(left)
        && !is_cjk(right)
        && !is_cjk_punctuation(left)
        && !is_cjk_punctuation(right)
        && !matches!(right, ',' | '.' | '!' | '?' | ';' | ':' | ')' | ']' | '}')
        && !matches!(left, '(' | '[' | '{')
}

fn recognize_tesseract(image: &ImageBuffer, language: &str, psm: Option<u8>) -> Result<String> {
    recognize_with_program(Path::new("tesseract"), image, language, psm)
}

fn ensure_tesseract_available() -> Result<()> {
    let status = Command::new("tesseract")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::TesseractUnavailable
            } else {
                Error::Ocr(format!("unable to start tesseract: {error}"))
            }
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Ocr(format!("tesseract exited with {status}")))
    }
}

fn recognize_with_program(
    program: &Path,
    image: &ImageBuffer,
    language: &str,
    psm: Option<u8>,
) -> Result<String> {
    let prepared = prepare_images(image)?;
    let modes = psm.map_or_else(|| vec![3, 6], |mode| vec![mode]);
    let mut best: Option<OcrCandidate> = None;
    let mut last_error = None;
    for (variant, image) in prepared.iter().enumerate() {
        let mut png = Vec::new();
        image.write_encoded(&mut png, OutputFormat::Png, 100)?;
        let threshold_method = if variant == 0 { 0 } else { 2 };
        for mode in &modes {
            match run_tesseract(program, &png, language, *mode, threshold_method) {
                Ok(candidate) => {
                    if best
                        .as_ref()
                        .is_none_or(|current| candidate.score() > current.score())
                    {
                        best = Some(candidate);
                    }
                }
                Err(Error::TesseractUnavailable) => return Err(Error::TesseractUnavailable),
                Err(error) => last_error = Some(error),
            }
        }
    }
    best.map(|candidate| candidate.text)
        .ok_or_else(|| last_error.unwrap_or(Error::OcrNoText))
}

#[derive(Debug)]
struct OcrCandidate {
    text: String,
    confidence: f32,
    characters: usize,
}

impl OcrCandidate {
    fn score(&self) -> f32 {
        self.confidence + (self.characters as f32).ln_1p() * 1.5
    }
}

fn run_tesseract(
    program: &Path,
    png: &[u8],
    language: &str,
    psm: u8,
    threshold_method: u8,
) -> Result<OcrCandidate> {
    let psm = psm.to_string();
    let mut child = Command::new(program)
        .args([
            "stdin", "stdout", "-l", language, "--psm", &psm, "--oem", "1", "--dpi", "300",
        ])
        .arg("-c")
        .arg(format!("thresholding_method={threshold_method}"))
        .arg("tsv")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::TesseractUnavailable
            } else {
                Error::Ocr(format!("unable to start tesseract: {error}"))
            }
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| Error::Ocr("tesseract stdin is unavailable".into()))?
        .write_all(png)
        .map_err(|error| Error::Ocr(format!("unable to send image to tesseract: {error}")))?;
    let output = child
        .wait_with_output()
        .map_err(|error| Error::Ocr(format!("unable to wait for tesseract: {error}")))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::Ocr(if message.is_empty() {
            format!("tesseract exited with {}", output.status)
        } else {
            message
        }));
    }
    parse_tsv(&output.stdout)
}

fn parse_tsv(output: &[u8]) -> Result<OcrCandidate> {
    let output = std::str::from_utf8(output)
        .map_err(|error| Error::Ocr(format!("tesseract returned invalid UTF-8: {error}")))?;
    let mut text = String::new();
    let mut current_line = None;
    let mut weighted_confidence = 0.0f32;
    let mut characters = 0usize;
    for row in output.lines().skip(1) {
        let columns: Vec<_> = row.splitn(12, '\t').collect();
        if columns.len() != 12 || columns[0] != "5" {
            continue;
        }
        let token = columns[11].trim();
        if token.is_empty() {
            continue;
        }
        let line = [columns[1], columns[2], columns[3], columns[4]]
            .map(|value| value.parse::<u32>().unwrap_or(0));
        if current_line.is_some_and(|current| current != line) {
            text.push('\n');
        } else if current_line.is_some() {
            text.push(' ');
        }
        current_line = Some(line);
        text.push_str(token);

        let weight = token.chars().filter(|value| !value.is_whitespace()).count();
        let confidence = columns[10].parse::<f32>().unwrap_or(0.0).max(0.0);
        weighted_confidence += confidence * weight as f32;
        characters += weight;
    }
    let text = normalize_text(text);
    if text.trim().is_empty() || characters == 0 {
        return Err(Error::OcrNoText);
    }
    Ok(OcrCandidate {
        text,
        confidence: weighted_confidence / characters as f32,
        characters,
    })
}

fn normalize_text(mut text: String) -> String {
    while matches!(text.chars().last(), Some('\r' | '\n' | '\u{000c}')) {
        text.pop();
    }
    remove_cjk_spacing(&text)
}

fn prepare_images(source: &ImageBuffer) -> Result<Vec<ImageBuffer>> {
    let pixels = source.width as f64 * source.height as f64;
    let scale_factor = (TARGET_OCR_SCALE / source.scale.max(1.0))
        .clamp(1.0, TARGET_OCR_SCALE)
        .min((MAX_UPSCALED_PIXELS / pixels.max(1.0)).sqrt().max(1.0));
    let width = (source.width as f64 * scale_factor).round().max(1.0) as u32;
    let height = (source.height as f64 * scale_factor).round().max(1.0) as u32;

    let mut rgba = Vec::with_capacity(source.width as usize * source.height as usize * 4);
    for pixel in source.pixels.chunks_exact(4) {
        let alpha = pixel[3] as u32;
        for channel in &pixel[..3] {
            rgba.push(((*channel as u32 * alpha + 255 * (255 - alpha)) / 255) as u8);
        }
        rgba.push(255);
    }
    let rgba = image::RgbaImage::from_raw(source.width, source.height, rgba)
        .ok_or_else(|| Error::Ocr("unable to prepare color image".into()))?;
    let mut rgba =
        image::imageops::resize(&rgba, width, height, image::imageops::FilterType::Lanczos3)
            .into_raw();

    let mean = rgba
        .chunks_exact(4)
        .map(|pixel| (pixel[0] as u64 * 77 + pixel[1] as u64 * 150 + pixel[2] as u64 * 29) / 256)
        .sum::<u64>()
        / (width as u64 * height as u64).max(1);
    if mean < 128 {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[0] = 255 - pixel[0];
            pixel[1] = 255 - pixel[1];
            pixel[2] = 255 - pixel[2];
        }
    }
    let color = ImageBuffer {
        width,
        height,
        pixels: rgba,
        scale: 1.0,
        region: source.region.clone(),
    };
    let mut luminance = color
        .pixels
        .chunks_exact(4)
        .map(|pixel| {
            ((pixel[0] as u32 * 77 + pixel[1] as u32 * 150 + pixel[2] as u32 * 29) / 256) as u8
        })
        .collect::<Vec<_>>();
    stretch_contrast(&mut luminance);
    let mut grayscale = ImageBuffer::new(width, height)?;
    for (gray, pixel) in luminance
        .into_iter()
        .zip(grayscale.pixels.chunks_exact_mut(4))
    {
        pixel.copy_from_slice(&[gray, gray, gray, 255]);
    }
    grayscale.region = source.region.clone();
    Ok(vec![color, grayscale])
}

fn stretch_contrast(pixels: &mut [u8]) {
    if pixels.is_empty() {
        return;
    }
    let mut histogram = [0usize; 256];
    for value in pixels.iter() {
        histogram[*value as usize] += 1;
    }
    let tail = pixels.len() / 100;
    let mut seen = 0usize;
    let low = histogram
        .iter()
        .position(|count| {
            seen += count;
            seen > tail
        })
        .unwrap_or(0) as u8;
    seen = 0;
    let high = histogram
        .iter()
        .rposition(|count| {
            seen += count;
            seen > tail
        })
        .unwrap_or(255) as u8;
    if high.saturating_sub(low) < 32 {
        return;
    }
    let range = (high - low) as u16;
    for value in pixels {
        *value = if *value <= low {
            0
        } else if *value >= high {
            255
        } else {
            ((*value - low) as u16 * 255 / range) as u8
        };
    }
}

fn remove_cjk_spacing(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if is_horizontal_space(chars[index]) {
            let start = index;
            while index < chars.len() && is_horizontal_space(chars[index]) {
                index += 1;
            }
            let previous = result.chars().next_back();
            let next = chars.get(index).copied();
            let joins_cjk = previous.zip(next).is_some_and(|(left, right)| {
                (is_cjk(left) && (is_cjk(right) || is_cjk_punctuation(right)))
                    || (is_cjk_punctuation(left) && is_cjk(right))
            });
            if !joins_cjk {
                result.extend(&chars[start..index]);
            }
            continue;
        }
        result.push(chars[index]);
        index += 1;
    }
    result
}

fn is_horizontal_space(value: char) -> bool {
    matches!(value, ' ' | '\t' | '\u{00a0}')
}

fn is_cjk(value: char) -> bool {
    matches!(
        value as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            | 0x30000..=0x323af
    )
}

fn is_cjk_punctuation(value: char) -> bool {
    matches!(value as u32, 0x3000..=0x303f)
        || matches!(
            value,
            '，' | '。'
                | '！'
                | '？'
                | '；'
                | '：'
                | '、'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
        )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn block(text: &str, confidence: f32, x: f32, y: f32) -> TextBlock {
        TextBlock {
            text: text.into(),
            confidence,
            x,
            y,
            width: 20.0,
            height: 10.0,
        }
    }

    #[test]
    fn paddle_model_search_prefers_explicit_directory() {
        assert_eq!(
            paddle_model_directories(
                Some(Path::new("/models")),
                Some(std::ffi::OsStr::new("/environment")),
                Some(std::ffi::OsStr::new("/xdg")),
                Some(std::ffi::OsStr::new("/home/test")),
                Some(Path::new("/usr/bin/dmsshot-rs")),
            ),
            vec![PathBuf::from("/models")]
        );
    }

    #[test]
    fn paddle_model_search_uses_xdg_then_install_prefix() {
        assert_eq!(
            paddle_model_directories(
                None,
                None,
                Some(std::ffi::OsStr::new("/data")),
                Some(std::ffi::OsStr::new("/home/test")),
                Some(Path::new("/opt/dmsshot/bin/dmsshot-rs")),
            ),
            vec![
                PathBuf::from("/data/dmsshot-rs/models"),
                PathBuf::from("/opt/dmsshot/share/dmsshot-rs/models"),
                PathBuf::from("/usr/share/dmsshot-rs/models"),
            ]
        );
    }

    #[test]
    fn paddle_blocks_are_ordered_and_joined_without_chinese_spaces() {
        let blocks = vec![
            block("world", 0.9, 50.0, 2.0),
            block("你", 0.9, 0.0, 20.0),
            block("好", 0.9, 25.0, 20.0),
            block("hello", 0.9, 0.0, 0.0),
        ];
        assert_eq!(assemble_paddle_text(blocks).unwrap(), "hello world\n你好");
    }

    #[test]
    fn paddle_blocks_drop_low_confidence_noise() {
        let blocks = vec![
            block("usable", 0.8, 0.0, 0.0),
            block("noise", 0.1, 30.0, 0.0),
        ];
        assert_eq!(assemble_paddle_text(blocks).unwrap(), "usable");
    }

    #[test]
    #[ignore = "requires PP-OCRv4 model files; set DMSSHOT_TEST_OCR_MODEL_DIR"]
    fn paddle_models_load_and_run_inference() {
        let directory = std::env::var_os("DMSSHOT_TEST_OCR_MODEL_DIR")
            .map(PathBuf::from)
            .expect("DMSSHOT_TEST_OCR_MODEL_DIR is required");
        let models = PaddleModelPaths::from_directory(directory);
        assert!(models.is_complete());
        let mut image = ImageBuffer::new(320, 120).unwrap();
        image.pixels.fill(255);
        assert!(matches!(
            recognize_paddle(&models, &image),
            Ok(_) | Err(Error::OcrNoText)
        ));
    }

    #[test]
    fn normalizes_only_trailing_ocr_control_characters() {
        assert_eq!(
            normalize_text(" first\nsecond\r\n\u{000c}".into()),
            " first\nsecond"
        );
    }

    #[test]
    fn removes_only_artificial_spacing_between_chinese_characters() {
        assert_eq!(
            normalize_text("你 好 世 界\n中文 OCR test\nA B\n你 ， 好！\n".into()),
            "你好世界\n中文 OCR test\nA B\n你，好！"
        );
    }

    #[test]
    fn preprocessing_upscales_and_inverts_dark_screenshots() {
        let mut image = ImageBuffer::new(2, 1).unwrap();
        image.pixels = vec![20, 20, 20, 255, 120, 120, 120, 255];
        let prepared = prepare_images(&image).unwrap();
        assert_eq!(prepared.len(), 2);
        assert_eq!((prepared[0].width, prepared[0].height), (4, 2));
        assert!(prepared[0].pixels[0] > prepared[0].pixels[4]);
        assert!(
            prepared
                .iter()
                .all(|image| image.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255))
        );
    }

    #[test]
    fn parses_tsv_lines_and_character_weighted_confidence() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90.0\t中\n\
5\t1\t1\t1\t1\t2\t10\t0\t10\t10\t80.0\t文\n\
5\t1\t1\t1\t2\t1\t0\t10\t30\t10\t70.0\thello\n";
        let candidate = parse_tsv(tsv.as_bytes()).unwrap();
        assert_eq!(candidate.text, "中文\nhello");
        assert_eq!(candidate.characters, 7);
        assert!((candidate.confidence - 74.285_71).abs() < 0.001);
    }

    #[test]
    fn invokes_engine_with_png_on_stdin() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("dmsshot-ocr-{}-{unique}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let engine = directory.join("fake-tesseract");
        fs::write(
            &engine,
            "#!/bin/sh\ninput=$(dd bs=8 count=1 2>/dev/null)\n[ \"$input\" = \"$(printf '\\211PNG\\r\\n\\032\\n')\" ] || exit 9\ncat >/dev/null\nprintf 'level\\tpage_num\\tblock_num\\tpar_num\\tline_num\\tword_num\\tleft\\ttop\\twidth\\theight\\tconf\\ttext\\n'\nprintf '5\\t1\\t1\\t1\\t1\\t1\\t0\\t0\\t10\\t10\\t95.0\\thello\\n'\nprintf '5\\t1\\t1\\t1\\t1\\t2\\t10\\t0\\t10\\t10\\t96.0\\t世\\n'\nprintf '5\\t1\\t1\\t1\\t1\\t3\\t20\\t0\\t10\\t10\\t97.0\\t界\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&engine).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&engine, permissions).unwrap();

        let image = ImageBuffer::new(2, 2).unwrap();
        let result = recognize_with_program(&engine, &image, "chi_sim+eng", Some(6)).unwrap();
        assert_eq!(result, "hello 世界");
        fs::remove_dir_all(directory).unwrap();
    }
}
