#!/usr/bin/env bash
# Builds the portable Swift camera core as a shared library for the desktop watcher.
#
# Unlike the Android script this does not cross-compile: the desktop shell is built on
# the machine it runs on, for that machine's triple. The Rust host discovers the result
# through `OPC_CORE_LIB_DIR`, or by looking in `.build/<configuration>/` itself.
set -euo pipefail

readonly PRODUCT="OpenPocketCineDesktop"

usage() {
    cat <<'EOF'
Usage: desktop-build-swift-core.sh [--configuration debug|release]

Environment overrides:
  SWIFT_EXECUTABLE   Path to the Swift binary to build with (default: swift).

Prints the directory holding the built library, which is what `OPC_CORE_LIB_DIR`
wants. On Windows use `swift build --product OpenPocketCineDesktop` directly; this
script is the Unix convenience wrapper.
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

configuration="debug"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --configuration)
            [ "$#" -ge 2 ] || fail "--configuration needs a value"
            configuration="$2"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            fail "unexpected argument: $1"
            ;;
    esac
done

case "$configuration" in
    debug | release) ;;
    *) fail "configuration must be debug or release, got: $configuration" ;;
esac

readonly repo_root="$(cd "$(dirname "$0")/.." && pwd)"
readonly swift="${SWIFT_EXECUTABLE:-swift}"
command -v "$swift" >/dev/null 2>&1 || fail "no Swift toolchain on PATH (set SWIFT_EXECUTABLE)"

"$swift" build \
    --package-path "$repo_root" \
    --product "$PRODUCT" \
    --configuration "$configuration"

library_dir="$("$swift" build --package-path "$repo_root" --configuration "$configuration" \
    --show-bin-path)"
[ -d "$library_dir" ] || fail "Swift reported a bin path that does not exist: $library_dir"

found=""
for extension in dylib so dll; do
    if [ -f "$library_dir/lib$PRODUCT.$extension" ] || [ -f "$library_dir/$PRODUCT.$extension" ]; then
        found="yes"
        break
    fi
done
[ -n "$found" ] || fail "built $PRODUCT but found no shared library in $library_dir"

printf '%s\n' "$library_dir"
