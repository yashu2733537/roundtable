# Android

Status: scaffold + UI ship in-repo; device builds need the NDK and one
`tauri android init` (see below). The desktop CI builds a signed APK/AAB
on every version tag.

## One-time setup (dev machine)

```bash
rustup target add aarch64-linux-android   # + armv7 + x86_64 for emulators
# Android SDK: platform-tools, one platform, NDK r27 (via Android Studio
# or: sdkmanager "platform-tools" "platforms;android-34" "ndk;27.0.12077973")
export ANDROID_HOME=~/Android/Sdk
export NDK_HOME=$ANDROID_HOME/ndk/27.0.12077973
npx tauri android init     # generates src-tauri/gen/android (committed)
npx tauri android dev     # USB device or emulator
```

## What the app does differently on phones

- **Single pane + bottom nav** (Chats / Rooms / Agents); the Inspector is
  a slide-up sheet; Settings is a dialog. Desktop keeps the 3-pane layout.
- **Keys in the Android Keystore** through the same `keyring` API the
  desktop uses (`android-native-keyring-store`, enabled for the Android
  target in `src-tauri/Cargo.toml`). `secrets.rs` is unchanged across
  platforms. Same rules: memory-only reads, never in SQLite/logs.
- **Foreground service** keeps room runs alive when the screen sleeps:
  `gen/android/.../RoomService.kt` posts a persistent notification while
  any driver runs and stops with it. The manifest requests
  `FOREGROUND_SERVICE_DATA_SYNC` and nothing else new.
- **Cleartext LAN**: `networkSecurityConfig` trusts cleartext only to
  RFC-1918 + Tailscale ranges (remote mode is plaintext HTTP by design —
  pairing + tokens still apply; prefer Tailscale). Public HTTPS stays
  default-trust.
- **Local models are out of scope**: cloud providers + remote mode only.
  The model picker hides the Ollama entry on Android.

## Manual checklist (before publishing an APK)

1. Fresh install → add provider → key shows "Saved" → relaunch → still saved.
2. Chat streams; Stop works; rotate the screen mid-stream (no crash, no dup).
3. Rooms: start a run, sleep the screen 2 min, wake — run continued, notification showed.
4. Pair with desktop over Tailscale (docs/remote.md); revoke from desktop → phone 401s with guidance.
5. Kill the app mid-run → desktop run continues; reopening the room shows the full log.
