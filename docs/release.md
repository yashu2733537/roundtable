# Release engineering

## One-time setup (maintainer machine)

Desktop updater keys (private key stays OUT of the repo):

```bash
npx tauri signer generate -w ~/.tauri-signing/roundtable -f --ci
# → ~/.tauri-signing/roundtable      (secret — back it up offline)
# → ~/.tauri-signing/roundtable.pub  (public — already in tauri.conf.json)
```

Android signing key (private key stays OUT of the repo):

```bash
keytool -genkeypair -v -keystore ~/.android-release.jks -alias roundtable \
  -keyalg RSA -keysize 2048 -validity 10000
base64 -w0 ~/.android-release.jks  # → ANDROID_KEYSTORE_BASE64 (below)
```

## CI secrets

| Secret | Source |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | contents of `~/.tauri-signing/roundtable` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | empty unless you set one at generate time |
| `ANDROID_KEYSTORE_BASE64` | `base64 -w0 ~/.android-release.jks` |
| `ANDROID_KEY_ALIAS` | `roundtable` (or your alias) |
| `ANDROID_STORE_PASSWORD` | keystore password |
| `ANDROID_KEY_PASSWORD` | key password |

## Publishing

1. Bump `version` in `src-tauri/tauri.conf.json` (and `Cargo.toml`s if you
   keep them in sync), update the README phase checklist.
2. Replace the updater `endpoints` placeholder (`YOU/roundtable`) with the
   real repo if you forked.
3. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
4. `.github/workflows/release.yml` builds Windows (MSI+NSIS), Linux
   (AppImage+deb), and Android (signed APK+AAB), then opens a **draft**
   release. Review the artifacts, publish the draft.
5. Desktop clients pick up the update in-app on next launch (updater dialog
   enabled, signature verified against the embedded pubkey).

## What CI does NOT do

- It cannot test the Android app on a device — sideload the APK and walk
  the manual checklist in `docs/android.md` before publishing.
- It cannot test pairing over a real LAN — walk `docs/remote.md` with a
  phone and a laptop on the same network (or Tailscale) before publishing.
