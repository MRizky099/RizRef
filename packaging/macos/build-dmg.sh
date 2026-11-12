#!/usr/bin/env bash
# Packages a built RizRef.app as the two macOS downloads, and checks them:
#
#   RizRef-<version>-macos-universal.dmg   the installer: drag to Applications
#   RizRef-<version>-macos-universal.zip   the portable build: run from anywhere
#
#   packaging/macos/build-dmg.sh [RizRef.app] [out-dir]
#       defaults: dist/macos/RizRef.app, dist/macos
#
# Build the app first with packaging/macos/build-app.sh (which checks the
# slices and the minimum OS). This script then:
#   - stages the app, a link to /Applications and "How to open RizRef.txt";
#   - writes a compressed, read-only HFS+ image (UDZO). HFS+ rather than APFS
#     so the image mounts on every supported macOS, 10.13 included;
#   - verifies the image's checksum (`hdiutil verify`), mounts it read-only
#     and checks the app inside verifies (`codesign`) and starts (`--version`);
#   - zips the app with `ditto`, which keeps the executable bit and the
#     signature;
#   - writes SHA-256 sums for both files.
#
# Signing: the app carries an ad-hoc signature (build-app.sh), which Apple
# Silicon requires to run at all. It is not a Developer ID signature and the
# image is not notarized — that needs a paid Apple Developer account — so
# Gatekeeper asks on first launch; the bundled text explains the steps.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
app="${1:-$root/dist/macos/RizRef.app}"
out="${2:-$root/dist/macos}"
cd "$root"

[ -d "$app/Contents/MacOS" ] || { echo "::error::no app bundle at $app (run build-app.sh)"; exit 1; }
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
base="RizRef-$version-macos-universal"
dmg="$out/$base.dmg"
zip="$out/$base.zip"
mkdir -p "$out"
rm -f "$dmg" "$zip"

staging=$(mktemp -d)
mount=$(mktemp -d)
cleanup() {
  hdiutil detach "$mount" -quiet 2>/dev/null || true
  rm -rf "$staging" "$mount"
}
trap cleanup EXIT

ditto "$app" "$staging/RizRef.app"
ln -s /Applications "$staging/Applications"
cp "packaging/macos/How to open RizRef.txt" "$staging/"

hdiutil create -quiet -volname "RizRef $version" -fs HFS+ -format UDZO \
  -imagekey zlib-level=9 -srcfolder "$staging" "$dmg"

failed=0
fail() { printf '::error::%s\n' "$*"; failed=1; }

hdiutil verify -quiet "$dmg" || fail "the image does not verify"
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mount" "$dmg"
for entry in "RizRef.app" "Applications" "How to open RizRef.txt"; do
  [ -e "$mount/$entry" ] || fail "the image has no $entry"
done
[ "$(readlink "$mount/Applications")" = "/Applications" ] || fail "the Applications link is wrong"
codesign --verify --strict "$mount/RizRef.app" || fail "the app in the image does not verify"
"$mount/RizRef.app/Contents/MacOS/rizref" --version || fail "the app in the image does not start"
hdiutil detach -quiet "$mount"

ditto -c -k --keepParent "$app" "$zip"

(cd "$out" && shasum -a 256 "$base.dmg" "$base.zip" > "$base.sha256")
cat "$out/$base.sha256"
ls -l "$dmg" "$zip"

if [ "$failed" -eq 0 ]; then
  echo "packaged $dmg and $zip"
fi
exit "$failed"
