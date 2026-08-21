# dmsshot-rs

Native Rust screenshot utility for niri. It uses Wayland screencopy and layer-shell directly; no
`grim` or `slurp` process is required.

## Relationship to DankMaterialShell

`dmsshot-rs` is an independent Rust rewrite of the screenshot module from
[DankMaterialShell (DMS)](https://github.com/AvengeMedia/DankMaterialShell). Its capture workflow,
region selector, scrolling capture, and stitching behavior are based on the original DMS
implementation. DankMaterialShell is developed by Avenge Media LLC and distributed under the MIT
License; see [LICENCE](LICENCE) for the applicable copyright and license notice.

## Build

```sh
cargo build --release
```

The resulting binary is `target/release/dmsshot-rs`. Building requires Rust 1.88 or newer, Wayland
development files, `pkg-config`, and `libxkbcommon`. At runtime, `wl-copy` provides persistent
Wayland clipboard ownership, `notify-send` sends the actionable completion notification, and
`xdg-open` opens captured images or their containing folders.
The optional `ocr` command requires the `tesseract` executable and the trained data for each
requested language.

### Arch Linux package

The included `PKGBUILD` declares Tesseract, Simplified and Traditional Chinese (horizontal and
vertical), and English trained data as dependencies. A normal pacman-managed installation places
the binary in `/usr/bin`:

```sh
makepkg -si
```

For a user-local installation in `~/.local/bin`, install the dependencies, build the same release
target, and install it as the current user:

```sh
sudo pacman -S --needed tesseract tesseract-data-chi_sim tesseract-data-chi_tra \
  tesseract-data-chi_sim_vert tesseract-data-chi_tra_vert tesseract-data-eng
makepkg -s
install -Dm755 target/release/dmsshot-rs "$HOME/.local/bin/dmsshot-rs"
```

Make sure `$HOME/.local/bin` is present in `PATH`. A PKGBUILD intentionally cannot install directly
into an arbitrary user's home directory: pacman packages are installed as root and must own stable
system paths.

## Usage

```sh
dmsshot-rs                 # interactive region
dmsshot-rs full            # focused output
dmsshot-rs all             # composite every output
dmsshot-rs output DP-1     # named output
dmsshot-rs window          # focused niri window
dmsshot-rs scroll          # interactive scrolling capture
dmsshot-rs ocr             # recognize text in an interactive region
dmsshot-rs list            # output information
```

Run `dmsshot-rs --help` for image format, destination, cursor, JSON, and stdout options. Region mode
uses Enter or Space to confirm, Escape to cancel, `P` to toggle the captured pointer, and Shift to
constrain a drag to a square. Scroll mode finishes with Enter or the Done button. Successful file
captures are copied and notified by default; use `--no-clipboard` or `--no-notify` to disable either
integration. Left-clicking a completion notification opens the captured image; its Open Folder
action opens the destination directory. Binary `--stdout` output remains free of clipboard and
notification side effects.

Unless `--dir` or `$DMS_SCREENSHOT_DIR` specifies another destination, screenshots are saved in the
user's XDG Pictures directory under `Screenshots` (normally `~/Pictures/Screenshots`). The directory
and any missing parents are created automatically. If the XDG Pictures directory is not configured,
`~/Pictures/Screenshots` is used; only environments without a home directory fall back to the
current directory.

The default format is PNG and the generated filename is
`screenshot_YYYY-MM-DD_HH-MM-SS.png`, using local time. Selecting JPEG or PPM changes the filename
extension accordingly, while `--filename` overrides the generated name.

## OCR

`dmsshot-rs ocr` opens the normal region selector and sends the selected pixels directly to
Tesseract as PNG data. It does not save a temporary or screenshot file. Recognized text is printed
to stdout and copied to the text clipboard by default; use `--no-clipboard` or `--no-notify` to
disable either integration. `--json` returns `status`, `text`, `width`, `height`, and `language`.

The default language is English and the default Tesseract page segmentation mode is 6. Select one
or more installed language models with `--lang`, and adjust segmentation with `--psm` when needed:

```sh
dmsshot-rs ocr
dmsshot-rs ocr --lang chi_sim+eng
dmsshot-rs ocr --lang jpn --psm 3 --json
```

Language names must match the output of `tesseract --list-langs`. For Chinese recognition, install
the `chi_sim` and/or `chi_tra` Tesseract trained-data packages provided by the operating system.

## GTK theme support

The region and scrolling overlays follow the current GTK theme for their background, text, and
accent colors. Theme discovery uses `GTK_THEME` first and then
`org.gnome.desktop.interface gtk-theme` from `gsettings`. Both GTK 3 and GTK 4 CSS files are
supported, including themes installed in the standard XDG data directories, `~/.themes`, and user
overrides under `$XDG_CONFIG_HOME/gtk-3.0` or `$XDG_CONFIG_HOME/gtk-4.0`.

The CSS reader understands common GTK color definitions and aliases such as `window_bg_color`,
`window_fg_color`, `accent_color`, and `theme_selected_bg_color`. GTK is not a build-time or runtime
library dependency. If the selected theme cannot be found or does not expose the required colors,
the overlay uses its built-in dark color scheme.

To override the detected theme for one invocation:

```sh
GTK_THEME=Adwaita:dark dmsshot-rs
```
