# macOS entitlements

Used by `packaging/sign_macos.py` (codesign's parser takes no XML comments,
so the reasons live here).

- `sonarad.entitlements` (releases, Developer ID): `com.apple.security.automation.apple-events`.
  sonarad pauses Music and Spotify through `/usr/bin/osascript`; under the hardened runtime
  Apple Events are refused (-1743, no prompt) without it. Library validation stays on: one
  Developer ID team signs sonarad and `libonnxruntime.dylib`.
- `sonarad-adhoc.entitlements` (LOCAL tests only, `sign --identity - --hardened`): the same, plus
  `com.apple.security.cs.disable-library-validation`. An ad-hoc signature has no team ID, so a
  hardened sonarad would refuse the ad-hoc `libonnxruntime.dylib` ("different Team IDs").
  release.yml fails a release whose sonarad carries it.
