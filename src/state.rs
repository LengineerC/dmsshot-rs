use std::{
    env, fs,
    path::{Path, PathBuf},
};

use chrono::Local;

use crate::{
    cli::OutputFormat,
    error::{Error, Result},
    imagebuf::Region,
};

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PersistentState {
    #[serde(default)]
    last_region: Region,
}

pub fn state_path() -> PathBuf {
    cache_home().join("dms/screenshot-state.json")
}

pub fn load_last_region() -> Region {
    fs::read(state_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PersistentState>(&bytes).ok())
        .map(|state| state.last_region)
        .unwrap_or_default()
}

pub fn save_last_region(region: Region) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&PersistentState {
        last_region: region,
    })?;
    fs::write(path, bytes)?;
    Ok(())
}

pub fn reset_last_region() -> Result<()> {
    save_last_region(Region::default())
}

pub fn output_directory(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return ensure_dir(path.to_path_buf());
    }
    if let Some(path) = env::var_os("DMS_SCREENSHOT_DIR").filter(|value| !value.is_empty()) {
        return ensure_dir(PathBuf::from(path));
    }
    if let Some(pictures) = pictures_directory() {
        return ensure_dir(pictures.join("Screenshots"));
    }
    Ok(home_dir().unwrap_or_else(|| PathBuf::from(".")))
}

pub fn generated_filename(format: OutputFormat) -> String {
    format!(
        "screenshot_{}.{}",
        Local::now().format("%Y-%m-%d_%H-%M-%S"),
        format.extension()
    )
}

fn ensure_dir(path: PathBuf) -> Result<PathBuf> {
    fs::create_dir_all(&path).map_err(|source| Error::CreateOutputDir {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

fn cache_home() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|path| path.join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn pictures_directory() -> Option<PathBuf> {
    let home = home_dir()?;
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("user-dirs.dirs");
    fs::read_to_string(config)
        .ok()
        .and_then(|text| parse_pictures_directory(&text, &home))
        .or_else(|| Some(home.join("Pictures")))
}

fn parse_pictures_directory(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text
        .lines()
        .find_map(|line| line.strip_prefix("XDG_PICTURES_DIR="))?
        .trim()
        .trim_matches('"')
        .replace("$HOME", home.to_str()?);
    (!value.is_empty()).then(|| PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_uses_selected_extension() {
        let png = generated_filename(OutputFormat::Png);
        assert!(png.starts_with("screenshot_"));
        assert!(png.ends_with(".png"));
        assert_eq!(png.len(), "screenshot_YYYY-MM-DD_HH-MM-SS.png".len());
        assert!(generated_filename(OutputFormat::Jpg).ends_with(".jpg"));
        assert!(generated_filename(OutputFormat::Ppm).ends_with(".ppm"));
    }

    #[test]
    fn parses_xdg_pictures_and_expands_home() {
        let home = Path::new("/home/tester");
        assert_eq!(
            parse_pictures_directory("XDG_PICTURES_DIR=\"$HOME/Pictures\"\n", home),
            Some(home.join("Pictures"))
        );
    }
}
