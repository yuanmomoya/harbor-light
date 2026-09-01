#!/bin/zsh
# 从 resources/logo.png 生成 macOS icns 和 Windows ico
set -euo pipefail
cd "$(dirname "$0")/.."
SRC="${1:-resources/logo.png}"
ICONSET=resources/AppIcon.iconset
if [[ ! -f "$SRC" ]]; then
  echo "找不到 $SRC" >&2
  exit 1
fi
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
sips -z 16 16     "$SRC" --out "$ICONSET/icon_16x16.png" >/dev/null
sips -z 32 32     "$SRC" --out "$ICONSET/icon_16x16@2x.png" >/dev/null
sips -z 32 32     "$SRC" --out "$ICONSET/icon_32x32.png" >/dev/null
sips -z 64 64     "$SRC" --out "$ICONSET/icon_32x32@2x.png" >/dev/null
sips -z 128 128   "$SRC" --out "$ICONSET/icon_128x128.png" >/dev/null
sips -z 256 256   "$SRC" --out "$ICONSET/icon_128x128@2x.png" >/dev/null
sips -z 256 256   "$SRC" --out "$ICONSET/icon_256x256.png" >/dev/null
sips -z 512 512   "$SRC" --out "$ICONSET/icon_256x256@2x.png" >/dev/null
sips -z 512 512   "$SRC" --out "$ICONSET/icon_512x512.png" >/dev/null
sips -z 1024 1024 "$SRC" --out "$ICONSET/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$ICONSET" -o resources/AppIcon.icns
sips -s format ico "$ICONSET/icon_256x256.png" --out resources/AppIcon.ico >/dev/null
rm -rf "$ICONSET"
echo "已生成 resources/AppIcon.icns 和 resources/AppIcon.ico"
