#!/bin/sh
set -eu
version=${FILETREE233_VERSION:-v0.1.0-alpha.1}
case "$version" in *[!A-Za-z0-9._-]*|"") echo "Invalid version" >&2; exit 1;; esac
release_base=${FILETREE233_RELEASE_BASE:-https://github.com/neko233-com/filetree233/releases/download}
case "$release_base" in *'?'*|*'#'*|*'@'*|*' '*) echo 'Invalid release base' >&2; exit 1;; esac
case "$release_base" in
    https://*) ;;
    http://127.0.0.1:*) port=${release_base#http://127.0.0.1:}; port=${port%%/*}; case "$port" in ''|*[!0-9]*) echo 'Invalid loopback port' >&2; exit 1;; esac ;;
    *) echo 'Release base must be HTTPS or loopback HTTP' >&2; exit 1;;
esac
base="${release_base%/}/$version"
case "$(uname -s)-$(uname -m)" in
    Linux-aarch64|Linux-arm64) name=filetree233-linux-arm64 ;;
    Linux-x86_64) name=filetree233-linux-x64 ;;
    Darwin-arm64) name=filetree233-macos-arm64 ;;
    Darwin-x86_64) name=filetree233-macos-x64 ;;
    *) echo 'Unsupported OS/architecture' >&2; exit 1 ;;
esac
install_dir=${FILETREE233_INSTALL_DIR:-"$HOME/.local/bin"}
tmp=$(mktemp -d)
staged=
cleanup() { [ -z "$staged" ] || rm -f "$staged"; rm -rf "$tmp"; }
trap cleanup EXIT HUP INT TERM
curl -fL --retry 3 "$base/$name" -o "$tmp/$name"
curl -fL --retry 3 "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
expected=$(awk -v n="$name" '$2 == n && length($1) == 64 {print $1}' "$tmp/SHA256SUMS")
[ "$(printf '%s\n' "$expected" | wc -l | tr -d ' ')" = 1 ] && [ -n "$expected" ] || { echo 'Missing or ambiguous checksum' >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$tmp/$name" | awk '{print $1}'); else actual=$(shasum -a 256 "$tmp/$name" | awk '{print $1}'); fi
[ "$actual" = "$expected" ] || { echo 'SHA-256 mismatch' >&2; exit 1; }
mkdir -p "$install_dir"
[ ! -d "$install_dir/filetree233" ] || { echo 'Destination is a directory' >&2; exit 1; }
staged=$(mktemp "$install_dir/.filetree233.XXXXXX")
install -m 755 "$tmp/$name" "$staged"
mv -f "$staged" "$install_dir/filetree233"
staged=
echo "Installed to $install_dir/filetree233"
