# Maintainer: LengineerC

pkgname=dmsshot-rs
pkgver=0.1.0
pkgrel=1
pkgdesc='Native Rust screenshot, scrolling capture, and OCR utility for niri'
arch=('x86_64')
url='https://github.com/LengineerC/dmsshot-rs'
license=('MIT')
depends=(
    'libnotify'
    'libxkbcommon'
    'tesseract'
    'tesseract-data-chi_sim'
    'tesseract-data-chi_tra'
    'tesseract-data-chi_sim_vert'
    'tesseract-data-chi_tra_vert'
    'tesseract-data-eng'
    'wayland'
    'wl-clipboard'
    'xdg-utils'
)
makedepends=(
    'cargo'
    'pkgconf'
)

build() {
    cd "$startdir"
    cargo build --release --locked
}

check() {
    cd "$startdir"
    cargo test --locked
}

package() {
    install -Dm755 "$startdir/target/release/dmsshot-rs" \
        "$pkgdir/usr/bin/dmsshot-rs"
    install -Dm644 "$startdir/LICENCE" \
        "$pkgdir/usr/share/licenses/$pkgname/LICENCE"
    install -Dm644 "$startdir/README.md" \
        "$pkgdir/usr/share/doc/$pkgname/README.md"
}
