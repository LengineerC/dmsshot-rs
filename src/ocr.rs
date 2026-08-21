use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    cli::OutputFormat,
    error::{Error, Result},
    imagebuf::ImageBuffer,
};

pub fn recognize(image: &ImageBuffer, language: &str, psm: u8) -> Result<String> {
    recognize_with_program(Path::new("tesseract"), image, language, psm)
}

pub fn ensure_available() -> Result<()> {
    let status = Command::new("tesseract")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::OcrUnavailable
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
    psm: u8,
) -> Result<String> {
    let mut png = Vec::new();
    image.write_encoded(&mut png, OutputFormat::Png, 100)?;
    let psm = psm.to_string();
    let mut child = Command::new(program)
        .args(["stdin", "stdout", "-l", language, "--psm", &psm])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::OcrUnavailable
            } else {
                Error::Ocr(format!("unable to start tesseract: {error}"))
            }
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| Error::Ocr("tesseract stdin is unavailable".into()))?
        .write_all(&png)
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
    let text = String::from_utf8(output.stdout)
        .map_err(|error| Error::Ocr(format!("tesseract returned invalid UTF-8: {error}")))?;
    let text = normalize_text(text);
    if text.trim().is_empty() {
        return Err(Error::OcrNoText);
    }
    Ok(text)
}

fn normalize_text(mut text: String) -> String {
    while matches!(text.chars().last(), Some('\r' | '\n' | '\u{000c}')) {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn normalizes_only_trailing_ocr_control_characters() {
        assert_eq!(
            normalize_text(" first\nsecond\r\n\u{000c}".into()),
            " first\nsecond"
        );
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
            "#!/bin/sh\ninput=$(dd bs=8 count=1 2>/dev/null)\n[ \"$input\" = \"$(printf '\\211PNG\\r\\n\\032\\n')\" ] || exit 9\n[ \"$1 $2 $3 $4 $5 $6\" = \"stdin stdout -l chi_sim+eng --psm 6\" ] || exit 8\nprintf 'hello 世界\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&engine).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&engine, permissions).unwrap();

        let image = ImageBuffer::new(2, 2).unwrap();
        let result = recognize_with_program(&engine, &image, "chi_sim+eng", 6).unwrap();
        assert_eq!(result, "hello 世界");
        fs::remove_dir_all(directory).unwrap();
    }
}
