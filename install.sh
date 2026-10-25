#!/bin/sh
# Installs malac and malac-control: prebuilt binaries from the latest GitHub
# release when one exists for this machine, otherwise built from source.
#
#   curl -fsSL https://raw.githubusercontent.com/lobirus/malac/main/install.sh | sh
#
# Environment:
#   MALAC_INSTALL_DIR  where the binaries go (default: ~/.local/bin)
#   MALAC_VERSION      release tag to install, e.g. v0.2.0 (default: latest)
#   MALAC_FROM_SOURCE  set to 1 to always build from source
#   MALAC_REF          branch or tag to build from source (default: main)
#   MALAC_SOURCE       build this local checkout instead of cloning
#   MALAC_RELEASES     release download base, for mirrors
set -eu

REPO="https://github.com/lobirus/malac.git"
RELEASES="${MALAC_RELEASES:-https://github.com/lobirus/malac/releases}"
INSTALL_DIR="${MALAC_INSTALL_DIR:-$HOME/.local/bin}"
REF="${MALAC_REF:-main}"
MIN_RUST="1.86"

say() { printf '%s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

version_at_least() {
    # $1 >= $2, comparing dotted numbers
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -t. -k1,1n -k2,2n -k3,3n | head -n1)" = "$2" ]
}

find_cargo() {
    if command -v cargo >/dev/null 2>&1; then
        return
    fi
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        PATH="$HOME/.cargo/bin:$PATH"
        return
    fi
    fail "Rust is not installed. Install it from https://rustup.rs (or 'brew install rust'), then run this again."
}

target_triple() {
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64) echo aarch64-apple-darwin ;;
        Darwin-x86_64) echo x86_64-apple-darwin ;;
        Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
        *) echo "" ;;
    esac
}

sha256() {
    if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1
}

# Downloads and installs a release archive. Returns 1 when there is no
# release for this machine, so the caller can build from source instead.
install_release() {
    target="$(target_triple)"
    [ -n "$target" ] || return 1
    command -v curl >/dev/null 2>&1 || return 1
    if [ -n "${MALAC_VERSION:-}" ]; then
        base="$RELEASES/download/$MALAC_VERSION"
    else
        base="$RELEASES/latest/download"
    fi
    curl -fsSL "$base/SHA256SUMS" -o "$work/SHA256SUMS" 2>/dev/null || return 1
    archive="$(grep -o "malac-[^ ]*-$target\.tar\.gz" "$work/SHA256SUMS" | head -n1)"
    [ -n "$archive" ] || return 1

    say "Downloading $archive..."
    curl -fsSL "$base/$archive" -o "$work/$archive" || fail "download failed: $base/$archive"
    expected="$(grep " $archive\$" "$work/SHA256SUMS" | cut -d' ' -f1)"
    [ "$(sha256 "$work/$archive")" = "$expected" ] || fail "checksum mismatch for $archive"
    tar -xzf "$work/$archive" -C "$work"
    dir="$work/${archive%.tar.gz}"
    mkdir -p "$INSTALL_DIR"
    install -m 755 "$dir/malac" "$dir/malac-control" "$INSTALL_DIR/"
}

install_from_source() {
    find_cargo
    rust="$(rustc --version | cut -d' ' -f2)"
    version_at_least "$rust" "$MIN_RUST" || fail "Rust $MIN_RUST or newer is required (found $rust); run 'rustup update'."

    if [ -n "${MALAC_SOURCE:-}" ]; then
        src="$MALAC_SOURCE"
    else
        command -v git >/dev/null 2>&1 || fail "git is required"
        say "Fetching malac ($REF)..."
        git clone --quiet --depth 1 --branch "$REF" "$REPO" "$work/src"
        src="$work/src"
    fi
    [ -f "$src/Cargo.toml" ] || fail "$src does not look like a malac checkout"

    say "Building malac with Rust $rust. This takes a few minutes..."
    CARGO_TARGET_DIR="$work/target" cargo build --release --locked \
        --manifest-path "$src/Cargo.toml" -p malac -p malac-control

    mkdir -p "$INSTALL_DIR"
    install -m 755 "$work/target/release/malac" "$work/target/release/malac-control" "$INSTALL_DIR/"
}

main() {
    case "$(uname -s)" in
        Darwin | Linux) ;;
        *) fail "unsupported system: $(uname -s)" ;;
    esac
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT INT TERM

    if [ -n "${MALAC_SOURCE:-}" ] || [ "${MALAC_FROM_SOURCE:-0}" = "1" ] || ! install_release; then
        install_from_source
    fi
    say ""
    say "Installed $("$INSTALL_DIR/malac" --version) to $INSTALL_DIR"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            say ""
            say "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
            say "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac

    say ""
    say "Next steps:"
    say "  malac provider add <name> --base-url <url> --model <model> --api-key-stdin --set-default"
    say "  malac            # start an interactive session"
    if [ "$(uname -s)" = "Darwin" ]; then
        say ""
        say "For desktop control, grant Screen Recording and Accessibility to your terminal app"
        say "in System Settings > Privacy & Security, or run 'malac-control serve' from a terminal"
        say "that has them."
    fi
}

main "$@"
