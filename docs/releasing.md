# Releasing

1. Bump `version` in the root `Cargo.toml` and in `editors/vscode/package.json`, then run `cargo build` so `Cargo.lock` follows.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. The `release` workflow builds:
   - `malac-<version>-aarch64-apple-darwin.tar.gz` and `malac-<version>-x86_64-apple-darwin.tar.gz`
   - `malac-<version>-macos.pkg`, a universal installer that puts both binaries in `/usr/local/bin`
   - `malac-<version>-x86_64-unknown-linux-gnu.tar.gz`
   - `malac-vscode-<version>.vsix`, the VS Code extension
   - `malac-<version>-android.apk`, the Android app signed with the release key
   - `SHA256SUMS`

   It then publishes them as a GitHub release with generated notes.

Bump `versionCode` and `versionName` in `apps/android/app/build.gradle.kts` and `MARKETING_VERSION` in `apps/ios/project.yml` with the rest, or Android refuses the new APK as an update.

`install.sh` picks up the latest release automatically and verifies the checksum, falling back to a source build when no archive matches the machine.

## Building release files locally

```bash
scripts/package.sh                                   # this machine only
scripts/package.sh aarch64-apple-darwin x86_64-apple-darwin --pkg --vscode
```

The second form needs `rustup target add x86_64-apple-darwin`. Files land in `dist/`. Upload them with `gh release create v0.2.0 dist/*` if CI is unavailable.

## Signing the macOS installer

Unsigned packages trigger Gatekeeper on download: users have to right-click and choose Open. To sign the package, set `MALAC_SIGN_IDENTITY` to a "Developer ID Installer" identity from your keychain before running `scripts/package.sh --pkg`. Then notarize it:

```bash
xcrun notarytool submit dist/malac-<version>-macos.pkg --keychain-profile <profile> --wait
xcrun stapler staple dist/malac-<version>-macos.pkg
```

## Signing the macOS binaries

macOS keeps `malac-control`'s Screen Recording and Accessibility grants across updates only when its designated requirement stays the same. An ad-hoc signature's requirement is a hash of the binary, so the release workflow signs `malac` (`io.malac.cli`) and `malac-control` (`io.malac.control`) with the project's self-signed code-signing certificate instead. The certificate comes from two repository secrets: `MACOS_CODESIGN_P12_BASE64` (the PKCS12 file, base64) and `MACOS_CODESIGN_PASSWORD`.

The workflow imports the certificate into a throwaway keychain and passes its SHA-1 as `MALAC_CODESIGN_IDENTITY` to `scripts/package.sh`. It then checks that every binary's requirement names that certificate (SHA-1 `41:00:12:ED:43:9E:0D:F6:D0:9E:75:B7:EB:B7:E7:B4:EB:09:AA:80`). Without the secrets the macOS job fails rather than ship ad-hoc binaries. Locally, `scripts/package.sh` signs ad hoc unless `MALAC_CODESIGN_IDENTITY` (and optionally `MALAC_CODESIGN_KEYCHAIN`) is set.

Keep the certificate and its password outside the repository and backed up. Binaries signed with a new certificate lose every user's grant once.

## Signing the Android app

The release workflow signs the APK with a PKCS12 keystore from two repository secrets: `ANDROID_KEYSTORE_BASE64` (the keystore, base64) and `ANDROID_KEYSTORE_PASSWORD`. The key alias is `malac`. The workflow checks the APK's certificate against the project's (SHA-256 `25:B7:1D:D6:94:F0:CC:57:4B:52:57:74:FA:5F:2F:33:5B:86:B2:09:EB:79:A2:40:D2:F1:27:83:B8:F1:BA:BA`), so a release can't go out signed with another key by mistake.

Keep the keystore and its password outside the repository and backed up. GitHub cannot give a secret back, and an APK signed with a new key cannot update the installed app: users would have to uninstall it and pair again.

To attach an APK to a release that has none, run the workflow by hand: `gh workflow run release.yml -f tag=v1.1.0`. It builds the APK from the default branch, adds it to that release and updates `SHA256SUMS`, leaving the other files as they are.

## Homebrew

`Formula/malac.rb` builds from `main` (`brew install --HEAD`). After the first tag, add a stable `url` pointing at `https://github.com/lobirus/malac/archive/refs/tags/v<version>.tar.gz` and its `sha256` (`curl -sL <url> | shasum -a 256`), so `brew install lobirus/malac/malac` works without `--HEAD`.
