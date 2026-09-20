#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
# shellcheck source=scripts/tools-versions.sh
source scripts/tools-versions.sh
target=${1:?Usage: release-package.sh TARGET DESTINATION [BINARY]}
destination=${2:?Destination required}
version=$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)
case "$target" in
aarch64-apple-darwin)
    floor='macOS 14'
    export MACOSX_DEPLOYMENT_TARGET=14.0
    ;;
x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu) floor='Ubuntu 24.04; glibc 2.39' ;;
*)
    echo "Unsupported release target: $target" >&2
    exit 1
    ;;
esac
host=$(rustup run "$RUST_TOOLCHAIN" rustc -vV | awk '/^host:/ {print $2}')
[ "$host" = "$target" ] || {
    echo 'Packaging requires native execution on the target' >&2
    exit 1
}
if [ "$#" -ge 3 ]; then
    binary=$3
else
    rustup run "$RUST_TOOLCHAIN" cargo build --locked --release --target "$target"
    binary="target/$target/release/tview"
fi
mkdir -p "$destination"
destination=$(CDPATH='' cd -- "$destination" && pwd)
name="tview-v$version-$target.tar.gz"
if [ -e "$destination/$name" ] || [ -e "$destination/$name.sha256" ]; then
    echo 'Refusing to replace an existing artifact' >&2
    exit 1
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/package" "$work/extracted" "$work/config"
cp "$binary" "$work/package/tview"
cp LICENSE.txt "$work/package/"
chmod 755 "$work/package/tview"
{
    printf 'tag: v%s\ncommit: %s\n' "$version" "$(git rev-parse HEAD)"
    printf 'compiler: %s\ntarget: %s\n' "$(rustup run "$RUST_TOOLCHAIN" rustc --version)" "$target"
    printf 'host: %s\nsupport floor: %s\nfeatures: saved-views,sqlite,clipboard\n' "$(uname -srm)" "$floor"
    if [ -n "$(git status --porcelain)" ]; then
        printf 'validation only: uncommitted source; commit identifies the base revision\n'
    fi
    if [ "$#" -ge 3 ]; then printf 'validation only: supplied binary, not a publishable release\n'; fi
} >"$work/package/BUILD-INFO.txt"
COPYFILE_DISABLE=1 tar -czf "$work/$name" -C "$work/package" tview LICENSE.txt BUILD-INFO.txt
tar -xzf "$work/$name" -C "$work/extracted"
[ "$("$work/extracted/tview" --version)" = "tview $version" ]
printf 'Name,Count\nalpha,2\n' | XDG_CONFIG_HOME="$work/config" "$work/extracted/tview" --no-view --output table - >"$work/actual"
printf 'Name   Count\nalpha      2\n' >"$work/expected"
diff -u "$work/expected" "$work/actual"
(cd "$work" && shasum -a 256 "$name" >"$name.sha256" && shasum -a 256 -c "$name.sha256")
# Move only verified archives into the artifact directory.
mv "$work/$name" "$work/$name.sha256" "$destination/"
