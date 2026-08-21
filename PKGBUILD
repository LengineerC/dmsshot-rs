# Maintainer: LengineerC

pkgname=dmsshot-rs
pkgver=0.1.0
pkgrel=2
pkgdesc='Native Rust screenshot, scrolling capture, and OCR utility for niri'
arch=('x86_64')
url='https://github.com/LengineerC/dmsshot-rs'
license=('MIT' 'Apache-2.0')
depends=(
    'libnotify'
    'libxkbcommon'
    'wayland'
    'wl-clipboard'
    'xdg-utils'
)
optdepends=(
    'tesseract: compatibility fallback for ocr --engine tesseract'
    'tesseract-data-chi_sim: Simplified Chinese for the Tesseract fallback'
    'tesseract-data-eng: English for the Tesseract fallback'
)
makedepends=(
    'cargo'
    'pkgconf'
)
source=(
    'ch_PP-OCRv4_det_infer.onnx::https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx'
    'ch_PP-OCRv4_rec_infer.onnx::https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_mobile.onnx'
    'ppocr_keys_v1.txt::https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/paddle/PP-OCRv4/rec/ch_PP-OCRv4_rec_mobile/ppocr_keys_v1.txt'
)
sha256sums=(
    'd2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9'
    '48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b'
    '28b2362ad4ab2dc38769aa72feb535e3a9ddb3fd2a7585a05920e6393b1dc7f7'
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
    install -Dm644 "$srcdir/ch_PP-OCRv4_det_infer.onnx" \
        "$pkgdir/usr/share/$pkgname/models/ch_PP-OCRv4_det_infer.onnx"
    install -Dm644 "$srcdir/ch_PP-OCRv4_rec_infer.onnx" \
        "$pkgdir/usr/share/$pkgname/models/ch_PP-OCRv4_rec_infer.onnx"
    install -Dm644 "$srcdir/ppocr_keys_v1.txt" \
        "$pkgdir/usr/share/$pkgname/models/ppocr_keys_v1.txt"
}
