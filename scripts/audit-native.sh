#!/usr/bin/env bash
# The pure-Rust rule, checked for one target: no vendored or linked C/C++.
#
#   scripts/audit-native.sh <target-triple> [build-dir]
#
# 1. Every `*-sys` crate in `cargo tree -e normal,build --target <triple>` must
#    be on the accepted list for that OS: binding crates that only declare an
#    OS API, or reach a system library through `dlopen` (README, "Dependency
#    audit").
# 2. Crates whose build script *can* compile C are checked for the feature
#    that keeps them from doing it (`wayland-sys` must be `dlopen`, never
#    linked; `wayland-backend` must not have `log`, its C shim).
# 3. With a build directory (`target/<profile>/build` after `cargo build`), no
#    build script may have left an object file or static library in its
#    `OUT_DIR`. The one exception is BLAKE3's compiler-flag probe, which
#    compiles an empty file and links nothing (README).
#
# Exit status 1 with a GitHub `::error::` line on any violation.
set -euo pipefail

target="${1:?usage: audit-native.sh <target-triple> [build-dir]}"
build_dir="${2:-}"

# Declarations-only / dlopen binding crates, per OS. The Win32 family appears
# on every host as a dependency of host-side build tools (proc-macros run on
# the build machine), and declares nothing for a non-Windows target.
windows_family='windows-sys|windows-targets|windows_[a-z0-9_]+'
case "$target" in
  *-windows-*)
    accepted="^(${windows_family}|renderdoc-sys)$" ;;
  *-linux-*)
    # linux-raw-sys: rustix's syscall declarations, pure Rust.
    # wayland-sys: libwayland reached through dlopen (checked below).
    accepted="^(${windows_family}|renderdoc-sys|linux-raw-sys|wayland-sys)$" ;;
  *-apple-*)
    # core-foundation-sys, objc-sys: declarations of system frameworks
    # (CoreFoundation, the Objective-C runtime), linked from the OS.
    accepted="^(${windows_family}|renderdoc-sys|core-foundation-sys|objc-sys)$" ;;
  *)
    printf '::error::no accepted list for target %s\n' "$target"
    exit 1 ;;
esac

failed=0
tree=$(cargo tree --workspace --target "$target" -e normal,build)
bindings=$(printf '%s\n' "$tree" \
  | grep -oE '[a-z0-9_]+(-[a-z0-9_]+)*[-_]sys v[0-9][^ ]*' \
  | sed -E 's/ v[0-9].*$//' | sort -u || true)
printf 'native-binding crates for %s:\n%s\n' "$target" "${bindings:-(none)}"
unexpected=$(printf '%s\n' "$bindings" | grep -Ev "$accepted" | grep -v '^$' || true)
if [ -n "$unexpected" ]; then
  printf '::error::%s: native bindings that are not accepted: %s\n' "$target" "$(echo $unexpected)"
  failed=1
fi

# Features that decide whether C is linked or compiled.
features_of() {
  cargo tree --workspace --target "$target" -e features -i "$1" 2>/dev/null \
    | grep -oE "$1 feature \"[a-z0-9_-]+\"" | sed -E 's/.*feature "(.*)"/\1/' | sort -u
}
if printf '%s\n' "$bindings" | grep -qx 'wayland-sys'; then
  if ! features_of wayland-sys | grep -qx 'dlopen'; then
    printf '::error::%s: wayland-sys is linked, not dlopen-ed\n' "$target"
    failed=1
  fi
fi
if printf '%s\n' "$tree" | grep -q 'wayland-backend v'; then
  if features_of wayland-backend | grep -qx 'log'; then
    printf '::error::%s: wayland-backend has `log`, which compiles a C shim\n' "$target"
    failed=1
  fi
fi

if [ -n "$build_dir" ]; then
  objects=$(find "$build_dir" -path '*/out/*' -type f \
      \( -name '*.o' -o -name '*.obj' -o -name '*.a' -o -name '*.lib' \) 2>/dev/null \
    | grep -Ev '/blake3-[0-9a-f]+/out/flag_check\.(o|obj)$' || true)
  if [ -n "$objects" ]; then
    printf '::error::%s: build scripts compiled native code:\n%s\n' "$target" "$objects"
    failed=1
  else
    printf 'no build script compiled native code (%s)\n' "$build_dir"
  fi
fi

exit "$failed"
