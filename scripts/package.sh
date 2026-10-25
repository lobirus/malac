#!/bin/sh
# Builds release archives into dist/.
#
#   scripts/package.sh                      # this machine's target
#   scripts/package.sh aarch64-apple-darwin x86_64-apple-darwin --pkg --vscode
#
# Each target becomes dist/malac-<version>-<target>.tar.gz. With --pkg on
# macOS, the Apple targets are merged into one universal binary and wrapped
# in dist/malac-<version>-macos.pkg, which installs to /usr/local/bin. Set
# MALAC_SIGN_IDENTITY to a "Developer ID Installer" identity to sign it.
# --vscode also packages the editor extension as malac-vscode-<version>.vsix.
set -eu

cd "$(dirname "$0")/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
dist="dist"
pkg=false
vscode=false
targets=""
for arg in "$@"; do
    case "$arg" in
        --pkg) pkg=true ;;
        --vscode) vscode=true ;;
        *) targets="$targets $arg" ;;
    esac
done
if [ -z "$targets" ]; then
    targets="$(rustc -vV | sed -n 's/^host: //p')"
fi

mkdir -p "$dist"
for target in $targets; do
    echo "Building $target..."
    cargo build --release --locked --target "$target" -p malac -p malac-control
    stage="$(mktemp -d)"
    name="malac-$version-$target"
    mkdir "$stage/$name"
    cp "target/$target/release/malac" "target/$target/release/malac-control" README.md LICENSE "$stage/$name/"
    tar -czf "$dist/$name.tar.gz" -C "$stage" "$name"
    rm -rf "$stage"
done

if $pkg; then
    [ "$(uname -s)" = "Darwin" ] || { echo "--pkg needs macOS" >&2; exit 1; }
    root="$(mktemp -d)"
    mkdir -p "$root/usr/local/bin"
    for bin in malac malac-control; do
        inputs=""
        for target in $targets; do
            case "$target" in *-apple-darwin) inputs="$inputs target/$target/release/$bin" ;; esac
        done
        # shellcheck disable=SC2086
        lipo -create $inputs -output "$root/usr/local/bin/$bin"
    done
    unsigned="$dist/malac-$version-macos-unsigned.pkg"
    pkgbuild --quiet --root "$root" --identifier io.malac.cli --version "$version" \
        --install-location / "$unsigned"
    if [ -n "${MALAC_SIGN_IDENTITY:-}" ]; then
        productsign --sign "$MALAC_SIGN_IDENTITY" "$unsigned" "$dist/malac-$version-macos.pkg"
        rm "$unsigned"
    else
        mv "$unsigned" "$dist/malac-$version-macos.pkg"
    fi
    rm -rf "$root"
fi

if $vscode; then
    (cd editors/vscode && npm ci --silent && npx --yes @vscode/vsce@3 package --out "../../$dist/malac-vscode-$version.vsix")
fi

(cd "$dist" && shasum -a 256 malac-*"$version"* > SHA256SUMS)
echo "Wrote:"
ls -1 "$dist"
