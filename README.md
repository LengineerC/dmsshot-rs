# dmsshot

Native Rust screenshot utility for niri. It uses Wayland screencopy and layer-shell directly; no
`grim` or `slurp` process is required.

## Build

```sh
cargo build --release
```

The resulting binary is `target/release/dmsshot`. Building requires Rust 1.88 or newer, Wayland
development files, `pkg-config`, and `libxkbcommon`. At runtime, `wl-copy` provides persistent
Wayland clipboard ownership and `notify-send` sends the completion notification.

## Usage

```sh
dmsshot                 # interactive region
dmsshot full            # focused output
dmsshot all             # composite every output
dmsshot output DP-1     # named output
dmsshot window          # focused niri window
dmsshot scroll          # interactive scrolling capture
dmsshot list            # output information
```

Run `dmsshot --help` for image format, destination, cursor, JSON, and stdout options. Region mode
uses Enter or Space to confirm, Escape to cancel, `P` to toggle the captured pointer, and Shift to
constrain a drag to a square. Scroll mode finishes with Enter or the Done button. Successful file
captures are copied and notified by default; use `--no-clipboard` or `--no-notify` to disable either
integration. Binary `--stdout` output remains free of clipboard and notification side effects.

The default destination is `$DMS_SCREENSHOT_DIR`, then the XDG Pictures `Screenshots` directory,
then the user's home directory.
