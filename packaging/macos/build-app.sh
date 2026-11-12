#!/usr/bin/env bash
# Builds RizRef.app — one universal bundle, Intel (macOS 10.13+) and Apple
# Silicon (macOS 11.0+) — and checks it. Runs on macOS with both Rust targets
# installed (`rustup target add x86_64-apple-darwin aarch64-apple-darwin`).
#
#   packaging/macos/build-app.sh [out-dir]        (default: dist/macos)
#
# Checks, each fatal:
#   - both slices are present (`lipo -archs`);
#   - each slice's minimum OS (`otool -l`: LC_BUILD_VERSION minos, or
#     LC_VERSION_MIN_MACOSX version) is exactly 10.13 / 11.0;
#   - each slice links only system libraries and frameworks (`otool -L`:
#     /usr/lib, /System/Library) — the pure-Rust rule at the binary level;
#   - Info.plist is valid (`plutil -lint`) and the bundle verifies after an
#     ad-hoc signature (`codesign`), which arm64 requires to run at all;
#   - the bundled binary starts (`--version`).
#
# The ad-hoc signature is not a Developer ID: Gatekeeper still asks on first
# open (there is no Apple Developer account, so no notarization either).
# packaging/macos/build-dmg.sh turns the bundle into the .dmg and the portable
# .zip; .github/workflows/release.yml runs both, on a version tag only.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
out="${1:-$root/dist/macos}"
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || { echo "::error::no workspace version in Cargo.toml"; exit 1; }

# target triple, minimum OS, lipo arch name
slices=("x86_64-apple-darwin 10.13 x86_64" "aarch64-apple-darwin 11.0 arm64")

for slice in "${slices[@]}"; do
  read -r triple minos _ <<<"$slice"
  echo "== building $triple for macOS $minos"
  MACOSX_DEPLOYMENT_TARGET="$minos" cargo build --release --locked -p rizref --target "$triple"
done

app="$out/RizRef.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
bin="$app/Contents/MacOS/rizref"
lipo -create -output "$bin" \
  target/x86_64-apple-darwin/release/rizref \
  target/aarch64-apple-darwin/release/rizref
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp crates/rizref/assets/rizref.icns crates/rizref/assets/rizref_document.icns \
  "$app/Contents/Resources/"

failed=0
fail() { printf '::error::%s\n' "$*"; failed=1; }

plutil -lint "$app/Contents/Info.plist" || fail "Info.plist does not lint"

archs=$(lipo -archs "$bin")
echo "slices: $archs"
for slice in "${slices[@]}"; do
  read -r triple minos arch <<<"$slice"
  case " $archs " in
    *" $arch "*) ;;
    *) fail "the universal binary has no $arch slice"; continue ;;
  esac
  found=$(otool -arch "$arch" -l "$bin" \
    | awk '/LC_BUILD_VERSION|LC_VERSION_MIN_MACOSX/ { cmd = 1 }
           cmd && ($1 == "minos" || $1 == "version") { print $2; exit }')
  echo "$arch: minimum macOS $found"
  [ "$found" = "$minos" ] || fail "$arch slice targets macOS ${found:-?}, expected $minos"
  foreign=$(otool -arch "$arch" -L "$bin" | tail -n +2 | awk '{ print $1 }' \
    | grep -Ev '^(/usr/lib/|/System/Library/)' || true)
  if [ -n "$foreign" ]; then
    fail "$arch slice links non-system libraries: $(echo $foreign)"
  else
    echo "$arch: links system libraries and frameworks only"
  fi
done

codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app" || fail "the bundle does not verify"

"$bin" --version || fail "the bundled binary does not start"

if [ "$failed" -eq 0 ]; then
  echo "built $app ($version)"
fi
exit "$failed"
