#!/bin/bash
# Builds the Windows version on Linux: dist/ChoDiLamIDE-<ver>-windows-x64.zip
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64-gcc, wine (runs fxc/fxc.exe, which
# compiles GPUI's HLSL shaders with Microsoft's d3dcompiler_47.dll), zip.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
VER=$(grep -m1 '^pkgver=' "$ROOT/packaging/arch/PKGBUILD" | cut -d= -f2)
cd "$ROOT"
GPUI_FXC_PATH="$HERE/fxc/fxc-wine" cargo build --release --target x86_64-pc-windows-gnu -p chodilamide-app -p chodilamide
# the Java helpers are platform independent: take them from the Linux build
JARS=packaging/arch/pkg/chodilamide-ide/usr/share/chodilamide
[ -f "$JARS/chodilamide-debugger.jar" ] || { echo "chạy makepkg trong packaging/arch trước (cần các jar)"; exit 1; }
OUT=$ROOT/dist/ChoDiLamIDE
rm -rf "$OUT"; mkdir -p "$OUT"
cp target/x86_64-pc-windows-gnu/release/chodilamide-ide.exe "$OUT/ChoDiLamIDE.exe"
cp target/x86_64-pc-windows-gnu/release/chodilamide.exe "$OUT/chodilamide-cli.exe"
cp "$JARS"/*.jar "$OUT/"
cp crates/app/assets/fonts/OFL.txt "$OUT/JetBrainsMono-OFL.txt"
cp "$HERE/README.txt" "$OUT/README.txt"
ZIP=$ROOT/dist/ChoDiLamIDE-$VER-windows-x64.zip
rm -f "$ZIP"
(cd "$ROOT/dist" && zip -qr "$ZIP" ChoDiLamIDE)
ls -la "$ZIP"
