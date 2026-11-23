#!/usr/bin/env bash
# Release packaging for toppingctl (#31). Called by .github/workflows/release.yml;
# tests/package.rs runs it locally.
#
#   scripts/package.sh archive <tag> <target> <bin-dir> <out-dir>
#       Writes <out-dir>/toppingctl-<tag>-<target>.tar.gz with the toppingctl binary
#       from <bin-dir>, README.md, CHANGELOG.md (if present), LICENSE-MIT,
#       LICENSE-APACHE and, for Linux targets, packaging/linux/ (the udev rule).
#       The dev tools hidsend/hidsniff are not shipped.
#
#   scripts/package.sh sums <dir>
#       Writes <dir>/SHA256SUMS covering every *.tar.gz in <dir>.
set -euo pipefail

# macOS tar: no AppleDouble ._* entries for files that carry extended attributes.
export COPYFILE_DISABLE=1

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

die() {
    echo "package.sh: $*" >&2
    exit 1
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        shasum -a 256 "$@"
    fi
}

# A tag or target becomes a file name: no path separators, no leading dot.
check_name() {
    [[ "$2" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "bad $1: '$2'"
}

archive() {
    [[ $# -eq 4 ]] || die "usage: archive <tag> <target> <bin-dir> <out-dir>"
    local tag="$1" target="$2" bin_dir="$3" out_dir="$4"
    check_name tag "$tag"
    check_name target "$target"
    [[ -f "$bin_dir/toppingctl" ]] || die "no toppingctl binary in $bin_dir"

    local name="toppingctl-${tag}-${target}"
    mkdir -p "$out_dir"
    # Global, so the EXIT trap still sees it after this function returns.
    stage="$(mktemp -d)"
    trap 'rm -rf "$stage"' EXIT

    local dest="$stage/$name"
    mkdir -p "$dest"
    install -m 0755 "$bin_dir/toppingctl" "$dest/toppingctl"
    local doc
    for doc in README.md LICENSE-MIT LICENSE-APACHE; do
        install -m 0644 "$root/$doc" "$dest/$doc"
    done
    if [[ -f "$root/CHANGELOG.md" ]]; then
        install -m 0644 "$root/CHANGELOG.md" "$dest/CHANGELOG.md"
    fi
    case "$target" in
        *-linux-*)
            mkdir -p "$dest/packaging/linux"
            install -m 0644 "$root/packaging/linux/70-toppingctl.rules" \
                "$root/packaging/linux/README.md" "$dest/packaging/linux/"
            ;;
    esac

    tar -C "$stage" -czf "$out_dir/$name.tar.gz" "$name"
    echo "$out_dir/$name.tar.gz"
}

sums() {
    [[ $# -eq 1 ]] || die "usage: sums <dir>"
    local dir="$1"
    (
        cd "$dir"
        shopt -s nullglob
        local files=(*.tar.gz)
        [[ ${#files[@]} -gt 0 ]] || die "no .tar.gz archives in $dir"
        sha256 "${files[@]}" >SHA256SUMS
        cat SHA256SUMS
    )
}

cmd="${1:-}"
shift || true
case "$cmd" in
    archive) archive "$@" ;;
    sums) sums "$@" ;;
    *) die "usage: package.sh archive <tag> <target> <bin-dir> <out-dir> | sums <dir>" ;;
esac
