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

## Usage

```sh
dmsshot-rs                 # interactive region
dmsshot-rs full            # focused output
dmsshot-rs all             # composite every output
dmsshot-rs output DP-1     # named output
dmsshot-rs window          # focused niri window
dmsshot-rs scroll          # interactive scrolling capture
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
