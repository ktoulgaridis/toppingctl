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
#       Writes <dir>/SHA256SUMS covering every *.tar.gz and *.deb in <dir>.
#
#   scripts/package.sh render aur <tag> <sums> <out-dir>
#       Renders packaging/aur/{toppingctl-bin,toppingctl}/{PKGBUILD,.SRCINFO} for <tag>
#       into <out-dir>/<pkg>/ (#22): pkgver, pkgrel=1 and the sha256 lines, from <sums>.
#       <sums> needs the two Linux archives and toppingctl-<tag>-source.tar.gz (GitHub's
#       tag archive, which the packages workflow downloads and hashes).
#       <out-dir> may be packaging/aur itself, to bump the in-repo files.
#
#   scripts/package.sh render brew <tag> <sums> <out-dir>
#       Renders packaging/brew/toppingctl.rb.in into <out-dir>/toppingctl.rb.
#
#   scripts/package.sh apt-repo <repo-dir> <deb>...
#       Adds the debs to the apt repo checkout <repo-dir> (pool/main/<l>/<pkg>/), then
#       regenerates dists/stable (Packages for amd64 and arm64, Release) and signs it
#       (InRelease, Release.gpg) with the gpg key named by $APT_SIGNING_KEY. Debs the
#       pool already has are skipped; when none is new, nothing is touched. A different
#       deb under a published name is an error. Needs apt-ftparchive (apt-utils) and gpg.
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
        local files=(*.tar.gz *.deb)
        [[ ${#files[@]} -gt 0 ]] || die "no .tar.gz archives or .debs in $dir"
        sha256 "${files[@]}" >SHA256SUMS
        cat SHA256SUMS
    )
}

# The sha256 of <name> in a SHA256SUMS file (text or binary-mode lines).
sum_of() {
    local sums="$1" name="$2" hash
    hash="$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1; exit }' "$sums")"
    [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || die "no sha256 for $name in $sums"
    echo "$hash"
}

# A release tag as a package version: v1.2.3 -> 1.2.3. Pre-release tags are not
# published to package managers (pkgver can't hold a '-').
version_of() {
    [[ "$1" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || die "bad tag: '$1' (want vX.Y.Z)"
    echo "${BASH_REMATCH[1]}"
}

# Writes stdin to <file> through a temp file, so <file> may also be the input.
write_to() {
    local tmp
    tmp="$(mktemp "$1.XXXXXX")"
    cat >"$tmp"
    mv "$tmp" "$1"
}

render_aur() {
    [[ $# -eq 3 ]] || die "usage: render aur <tag> <sums> <out-dir>"
    local tag="$1" sums="$2" out="$3" ver x86 arm src pkg old
    ver="$(version_of "$tag")"
    x86="$(sum_of "$sums" "toppingctl-$tag-x86_64-unknown-linux-gnu.tar.gz")"
    arm="$(sum_of "$sums" "toppingctl-$tag-aarch64-unknown-linux-gnu.tar.gz")"
    src="$(sum_of "$sums" "toppingctl-$tag-source.tar.gz")"
    local tab=$'\t'
    for pkg in toppingctl-bin toppingctl; do
        local in="$root/packaging/aur/$pkg"
        old="$(sed -n 's/^pkgver=//p' "$in/PKGBUILD")"
        [[ "$old" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$in/PKGBUILD: bad pkgver '$old'"
        local old_re="${old//./\\.}"
        mkdir -p "$out/$pkg"
        sed -E \
            -e "s/^pkgver=.*/pkgver=$ver/" \
            -e "s/^pkgrel=.*/pkgrel=1/" \
            -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$x86')/" \
            -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('$arm')/" \
            -e "s/^sha256sums=.*/sha256sums=('$src')/" \
            "$in/PKGBUILD" | write_to "$out/$pkg/PKGBUILD"
        # .SRCINFO spells the version out in the source URLs: swap it only there,
        # and only where it stands as a version (after v or -, before - . or /).
        sed -E \
            -e "s/^${tab}pkgver = .*/${tab}pkgver = $ver/" \
            -e "s/^${tab}pkgrel = .*/${tab}pkgrel = 1/" \
            -e "s/^${tab}sha256sums_x86_64 = .*/${tab}sha256sums_x86_64 = $x86/" \
            -e "s/^${tab}sha256sums_aarch64 = .*/${tab}sha256sums_aarch64 = $arm/" \
            -e "s/^${tab}sha256sums = .*/${tab}sha256sums = $src/" \
            -e "/^${tab}source(_[a-z0-9_]+)? = /s#([v-])$old_re([-./])#\\1$ver\\2#g" \
            "$in/.SRCINFO" | write_to "$out/$pkg/.SRCINFO"
    done
    echo "$out/toppingctl-bin" "$out/toppingctl"
}

render_brew() {
    [[ $# -eq 3 ]] || die "usage: render brew <tag> <sums> <out-dir>"
    local tag="$1" sums="$2" out="$3" ver target key
    ver="$(version_of "$tag")"
    local args=(-e "s/@VERSION@/$ver/g")
    for target in aarch64-apple-darwin x86_64-apple-darwin \
        x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
        key="$(echo "$target" | tr 'a-z-' 'A-Z_')"
        args+=(-e "s/@SHA256_${key}@/$(sum_of "$sums" "toppingctl-$tag-$target.tar.gz")/g")
    done
    mkdir -p "$out"
    sed "${args[@]}" "$root/packaging/brew/toppingctl.rb.in" | write_to "$out/toppingctl.rb"
    if grep -q '@[A-Z0-9_]*@' "$out/toppingctl.rb"; then
        die "unfilled placeholder in $out/toppingctl.rb"
    fi
    echo "$out/toppingctl.rb"
}

render() {
    local kind="${1:-}"
    shift || true
    case "$kind" in
        aur) render_aur "$@" ;;
        brew) render_brew "$@" ;;
        *) die "usage: render aur|brew <tag> <sums> <out-dir>" ;;
    esac
}

# The apt repo's suite and architectures, and its public key's file name.
APT_SUITE=stable
APT_ARCHES=(amd64 arm64)
APT_KEYRING=ktoulgaridis-archive-keyring.gpg

apt_repo() {
    [[ $# -ge 2 ]] || die "usage: apt-repo <repo-dir> <deb>..."
    local repo="$1"
    shift
    [[ -d "$repo" ]] || die "no apt repo checkout at $repo"

    # Check every deb before writing anything.
    local deb base pkg dest new=()
    for deb in "$@"; do
        base="$(basename "$deb")"
        [[ -f "$deb" ]] || die "no deb at $deb"
        [[ "$base" =~ ^([a-z0-9][a-z0-9+.-]*)_[^_/]+_[a-z0-9]+\.deb$ ]] ||
            die "not a <pkg>_<version>_<arch>.deb name: $base"
        pkg="${BASH_REMATCH[1]}"
        dest="$repo/pool/main/${pkg:0:1}/$pkg/$base"
        if [[ -e "$dest" ]]; then
            cmp -s "$deb" "$dest" || die "$base is already published with other contents"
            echo "apt repo: $base already published"
        else
            new+=("$deb")
        fi
    done
    if [[ ${#new[@]} -eq 0 ]]; then
        echo "apt repo: unchanged"
        return 0
    fi
    local key="${APT_SIGNING_KEY:-}"
    [[ -n "$key" ]] || die "APT_SIGNING_KEY (the signing key's fingerprint) is not set"

    for deb in "${new[@]}"; do
        base="$(basename "$deb")"
        pkg="${base%%_*}"
        install -D -m 0644 "$deb" "$repo/pool/main/${pkg:0:1}/$pkg/$base"
        echo "apt repo: added $base"
    done

    (
        cd "$repo"
        local dists="dists/$APT_SUITE" arch
        for arch in "${APT_ARCHES[@]}"; do
            mkdir -p "$dists/main/binary-$arch"
            apt-ftparchive --arch "$arch" packages pool >"$dists/main/binary-$arch/Packages"
            gzip -9nkf "$dists/main/binary-$arch/Packages"
        done
        rm -f "$dists/Release" "$dists/InRelease" "$dists/Release.gpg"
        apt-ftparchive \
            -o APT::FTPArchive::Release::Origin=ktoulgaridis \
            -o APT::FTPArchive::Release::Label=ktoulgaridis \
            -o APT::FTPArchive::Release::Suite="$APT_SUITE" \
            -o APT::FTPArchive::Release::Codename="$APT_SUITE" \
            -o APT::FTPArchive::Release::Architectures="${APT_ARCHES[*]}" \
            -o APT::FTPArchive::Release::Components=main \
            release "$dists" | write_to "$dists/Release"
        gpg --batch --yes --local-user "$key" --digest-algo SHA512 \
            --clearsign --output "$dists/InRelease" "$dists/Release"
        gpg --batch --yes --local-user "$key" --digest-algo SHA512 \
            --armor --detach-sign --output "$dists/Release.gpg" "$dists/Release"
        # The public key users fetch (binary, for signed-by=), and no Jekyll on Pages.
        gpg --batch --export "$key" >"$APT_KEYRING"
        [[ -s "$APT_KEYRING" ]] || die "no public key exported for $key"
        touch .nojekyll
    )
}

cmd="${1:-}"
shift || true
case "$cmd" in
    archive) archive "$@" ;;
    sums) sums "$@" ;;
    render) render "$@" ;;
    apt-repo) apt_repo "$@" ;;
    *) die "usage: package.sh archive <tag> <target> <bin-dir> <out-dir> | sums <dir>" \
        "| render aur|brew <tag> <sums> <out-dir> | apt-repo <repo-dir> <deb>..." ;;
esac
