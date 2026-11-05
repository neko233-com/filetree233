#!/usr/bin/env bash
# One-command local packaging for the macOS and Linux targets in
# .github/workflows/release.yml.
#
#   macOS: cargo build --features gui -> .app bundle + unsigned DMG
#   Linux: CLI-only build            -> tar.gz (+ the existing install.sh)
#
# Set SKIP_BUILD=1 when the release binary is already built, as CI does.
# Nothing is signed or notarized, so Gatekeeper warns on first open.
set -euo pipefail
cd "$(dirname "$0")/.."

target="${TARGET:-$(rustc -Vv | awk '/^Host:/ {print $2}')}"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$version" ] || { echo 'could not read the version from Cargo.toml' >&2; exit 1; }
short="${version%%-*}"
case "$target" in
aarch64-*) arch=arm64 ;;
x86_64-*) arch=x64 ;;
*) echo "unsupported target $target" >&2; exit 1 ;;
 esac

mkdir -p dist

case "$(uname -s)" in
Darwin)
  asset="filetree233-macos-$arch"
  if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --locked --features gui --target "$target"
  fi
  bin="target/$target/release/filetree233"
  [ -x "$bin" ] || { echo "expected $bin to exist" >&2; exit 1; }

  bundle="Filetree233.app"
  stage="dist/stage-$arch"
  rm -rf "$stage" "dist/$asset.dmg"
  mkdir -p "$stage/$bundle/Contents/MacOS" "$stage/$bundle/Contents/Resources"
  sed -e "s/@VERSION@/$short/" -e "s/@BUILD@/$short/" \
    packaging/macos/Info.plist >"$stage/$bundle/Contents/Info.plist"
  echo "APPL????" >"$stage/$bundle/Contents/PkgInfo"
  cp "$bin" "$stage/$bundle/Contents/MacOS/filetree233"
  chmod +x "$stage/$bundle/Contents/MacOS/filetree233"
  cp packaging/macos/Filetree233.icns "$stage/$bundle/Contents/Resources/Filetree233.icns"
  ln -s /Applications "$stage/Applications"
  # A drag-to-Applications image; unsigned, so right-click then Open the first time.
  hdiutil create -quiet -volname "filetree233 $short" -srcfolder "$stage" -ov \
    -format UDZO "dist/$asset.dmg"
  rm -rf "$stage"

  echo "packaged for $target (version $version):"
  (cd dist && shasum -a 256 "$asset.dmg")
  ;;
Linux)
  asset="filetree233-linux-$arch"
  # No gui feature: Linux releases stay CLI-only so they need no display server libraries.
  if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --locked --target "$target"
  fi
  bin="target/$target/release/filetree233"
  [ -x "$bin" ] || { echo "expected $bin to exist" >&2; exit 1; }

  stage="dist/$asset"
  rm -rf "$stage" "dist/$asset.tar.gz"
  mkdir -p "$stage"
  cp "$bin" "$stage/filetree233"
  for document in README.md CHANGELOG.md RELEASE_NOTES.md; do
    if [ -f "$document" ]; then cp "$document" "$stage/"; fi
  done
  cp scripts/install.sh "$stage/install.sh"
  tar -czf "dist/$asset.tar.gz" -C dist "$asset"
  rm -rf "$stage"

  echo "packaged for $target (version $version):"
  (cd dist && sha256sum "$asset.tar.gz")
  ;;
*)
  echo "packaging runs on macOS or Linux hosts; use scripts/package.ps1 on Windows" >&2
  exit 1
  ;;
esac
