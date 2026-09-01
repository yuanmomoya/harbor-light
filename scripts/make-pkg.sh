#!/bin/zsh
# 生成可双击安装的 dist/HarborLight.pkg
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
APP="${ROOT}/dist/HarborLight.app"
if [[ ! -d "$APP" ]]; then
  echo "请先打包 App：make package 或 ./scripts/package.sh" >&2
  exit 1
fi

STAGE="${ROOT}/dist/pkg-root"
SCRIPTS="${ROOT}/dist/pkg-scripts"
COMPONENT="${ROOT}/dist/HarborLight-component.pkg"
PRODUCT="${ROOT}/dist/HarborLight.pkg"
DISTXML="${ROOT}/dist/distribution.xml"

rm -rf "$STAGE" "$SCRIPTS"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/HarborLight.app"

mkdir -p "$SCRIPTS"
cp "${ROOT}/scripts/pkg/postinstall" "$SCRIPTS/postinstall"
cp "${ROOT}/scripts/pkg/preinstall" "$SCRIPTS/preinstall"
chmod 755 "$SCRIPTS/postinstall" "$SCRIPTS/preinstall"

# 禁止 PackageKit 把 App「重定位」到仓库里已有的 dist/HarborLight.app
pkgbuild \
  --root "$STAGE" \
  --install-location /Applications \
  --component-plist "${ROOT}/resources/installer/component.plist" \
  --scripts "$SCRIPTS" \
  --identifier com.harborlight.app \
  --version 0.1.0 \
  --ownership recommended \
  "$COMPONENT"

ARCHS=$(lipo -archs "$APP/Contents/MacOS/harbor-light" | tr -s ' ' ',' | sed 's/,$//')
if [[ -z "$ARCHS" ]]; then
  ARCHS="arm64"
fi

cat > "$DISTXML" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
    <title>Harbor Light</title>
    <organization>com.harborlight</organization>
    <options customize="never" require-scripts="false" hostArchitectures="${ARCHS}"/>
    <welcome file="welcome.html" mime-type="text/html"/>
    <pkg-ref id="com.harborlight.app"/>
    <choices-outline>
        <line choice="default">
            <line choice="com.harborlight.app"/>
        </line>
    </choices-outline>
    <choice id="default"/>
    <choice id="com.harborlight.app" visible="false">
        <pkg-ref id="com.harborlight.app"/>
    </choice>
    <pkg-ref id="com.harborlight.app" version="0.1.0" onConclusion="none">HarborLight-component.pkg</pkg-ref>
</installer-gui-script>
EOF

productbuild \
  --distribution "$DISTXML" \
  --resources "${ROOT}/resources/installer" \
  --package-path "${ROOT}/dist" \
  "$PRODUCT"

rm -rf "$STAGE" "$SCRIPTS" "$COMPONENT" "$DISTXML"
echo "已生成可双击安装包：${PRODUCT}"
