# Releasing

1. Bump `version` in the root `Cargo.toml` and run `cargo build` so `Cargo.lock` follows.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. The `release` workflow builds:
   - `malac-<version>-aarch64-apple-darwin.tar.gz` and `malac-<version>-x86_64-apple-darwin.tar.gz`
   - `malac-<version>-macos.pkg`, a universal installer that puts both binaries in `/usr/local/bin`
   - `malac-<version>-x86_64-unknown-linux-gnu.tar.gz`
   - `SHA256SUMS`

   It then publishes them as a GitHub release with generated notes.

`install.sh` picks up the latest release automatically and verifies the checksum, falling back to a source build when no archive matches the machine.

## Building release files locally

```bash
scripts/package.sh                                   # this machine only
scripts/package.sh aarch64-apple-darwin x86_64-apple-darwin --pkg
```

The second form needs `rustup target add x86_64-apple-darwin`. Files land in `dist/`. Upload them with `gh release create v0.2.0 dist/*` if CI is unavailable.

## Signing the macOS installer

Unsigned packages trigger Gatekeeper on download: users have to right-click and choose Open. To sign the package, set `MALAC_SIGN_IDENTITY` to a "Developer ID Installer" identity from your keychain before running `scripts/package.sh --pkg`. Then notarize it:

```bash
xcrun notarytool submit dist/malac-<version>-macos.pkg --keychain-profile <profile> --wait
xcrun stapler staple dist/malac-<version>-macos.pkg
```

## Homebrew

`Formula/malac.rb` builds from `main` (`brew install --HEAD`). After the first tag, add a stable `url` pointing at `https://github.com/lobirus/malac/archive/refs/tags/v<version>.tar.gz` and its `sha256` (`curl -sL <url> | shasum -a 256`), so `brew install lobirus/malac/malac` works without `--HEAD`.
