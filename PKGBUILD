# Maintainer: LengineerC

pkgname=dmsshot-rs
pkgver=0.1.0
pkgrel=4
pkgdesc='Native Rust screenshot, scrolling capture, and OCR utility for niri'
arch=('x86_64')
url='https://github.com/LengineerC/dmsshot-rs'
license=('MIT' 'Apache-2.0')
# GCC LTO objects produced for clipper-sys cannot be consumed by rust-lld.
# Cargo's release profile still applies thin LTO to the Rust code.
options=(!lto)
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
    'ch_ppocr_mobile_v2.0_cls_infer.onnx::https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_mobile.onnx'
    'ch_PP-OCRv4_rec_infer.onnx::https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_mobile.onnx'
)
sha256sums=(
    'd2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9'
    'e47acedf663230f8863ff1ab0e64dd2d82b838fceb5957146dab185a89d6215c'
    '48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b'
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
    install -Dm644 "$srcdir/ch_ppocr_mobile_v2.0_cls_infer.onnx" \
        "$pkgdir/usr/share/$pkgname/models/ch_ppocr_mobile_v2.0_cls_infer.onnx"
    install -Dm644 "$srcdir/ch_PP-OCRv4_rec_infer.onnx" \
        "$pkgdir/usr/share/$pkgname/models/ch_PP-OCRv4_rec_infer.onnx"
}
