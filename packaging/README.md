# Packaging

How toppingctl reaches package managers (#22). Every Linux package installs the binary,
the udev rule at `/usr/lib/udev/rules.d/70-toppingctl.rules` (from
[`linux/`](linux/README.md)) and both licences.

| Directory | What | Published to |
|---|---|---|
| [`aur/toppingctl-bin/`](aur/toppingctl-bin/PKGBUILD) | PKGBUILD + .SRCINFO from the release archives (x86_64, aarch64) | AUR `toppingctl-bin` |
| [`aur/toppingctl/`](aur/toppingctl/PKGBUILD) | PKGBUILD + .SRCINFO built from source with cargo | AUR `toppingctl` |
| [`brew/toppingctl.rb.in`](brew/toppingctl.rb.in) | Formula template, prebuilt archives for macOS and Linux | `ktoulgaridis/homebrew-tap` |
| `[package.metadata.deb]` in [`Cargo.toml`](../Cargo.toml) | cargo-deb metadata (amd64, arm64) | release assets + `ktoulgaridis/apt` |

## The release flow

1. A `v*` tag runs [`release.yml`](../.github/workflows/release.yml): it builds the four
   archives and the two `.deb`s, and drafts a release with them and `SHA256SUMS`.
2. Kyriakos reviews the draft and publishes it.
3. Publishing runs [`packages.yml`](../.github/workflows/packages.yml):
   - `render` (no secrets) downloads `SHA256SUMS` and the debs, hashes GitHub's tag
     archive for the source package, and renders the AUR files and the formula with
     `scripts/package.sh render`;
   - `aur` pushes both PKGBUILDs to AUR (skipped, with a notice, when `AUR_SSH_KEY` is
     empty);
   - `brew` commits `Formula/toppingctl.rb` to the tap;
   - `apt` adds the debs to the apt repo, regenerates its indexes and signs them;
   - `check-aur`, `check-apt`, `check-brew` install from each the way a user would and
     check `toppingctl --version` (and, on Linux, the udev rule).

   Nothing is committed or pushed unless a file changed, so re-running the workflow
   ("Re-run jobs") for the same version is a no-op. Pre-releases are not published.

Rendering by hand:

```sh
scripts/package.sh render aur  v0.2.0 SHA256SUMS out/aur    # out/aur/<pkg>/{PKGBUILD,.SRCINFO}
scripts/package.sh render brew v0.2.0 SHA256SUMS out/brew   # out/brew/toppingctl.rb
```

`render aur` needs `toppingctl-<tag>-source.tar.gz` in `SHA256SUMS`: the sha256 of
`https://github.com/ktoulgaridis/toppingctl/archive/refs/tags/<tag>.tar.gz`. It always
sets `pkgrel=1`; bump `pkgrel` by hand for a packaging-only fix. Passing `packaging/aur`
as the output directory bumps the in-repo files.

## Installing from the in-repo PKGBUILD

The files in `packaging/aur/` are ready to build as they are, for the version they name
(bump them with `render aur` after a release). Until the AUR packages exist, or to
build without an AUR helper:

```sh
git clone git@github.com:ktoulgaridis/toppingctl.git
cd toppingctl/packaging/aur/toppingctl-bin   # or packaging/aur/toppingctl, from source
makepkg -si
```

## One-time setup (Kyriakos)

Agents don't do these: they need accounts, keys and repo admin.

### 1. AUR

1. Log in to <https://aur.archlinux.org> and generate a key used only for CI:
   `ssh-keygen -t ed25519 -N '' -C 'toppingctl CI' -f aur-ci`.
2. Add `aur-ci.pub` under *My Account → SSH Public Key*.
3. Store the private key: `gh secret set AUR_SSH_KEY --env release -R ktoulgaridis/toppingctl < aur-ci`,
   then delete `aur-ci` locally.

`toppingctl` and `toppingctl-bin` don't exist on AUR yet: the first push creates both
package bases.

**Turning the AUR push on later.** With `AUR_SSH_KEY` empty or unset, the `aur` job
skips the push with a notice and `check-aur` installs from the rendered PKGBUILDs
instead. Set the secret (step 3) and re-run the workflow for the latest release; from
then on `check-aur` installs from AUR.

### 2. The two repos

```sh
gh repo create ktoulgaridis/homebrew-tap --public --add-readme \
  --description "Homebrew tap: brew install ktoulgaridis/tap/toppingctl"
gh repo create ktoulgaridis/apt --public --add-readme \
  --description "Signed apt repository for toppingctl"
```

Then turn on GitHub Pages for `ktoulgaridis/apt`: *Settings → Pages → Deploy from a
branch → `main`, `/ (root)`*. It serves `https://ktoulgaridis.github.io/apt`. Leave `main`
unprotected on both repos (or allow the token to push), since the workflow pushes to it.

### 3. The apt signing key

A dedicated key that signs only the apt repo, with no passphrase (it lives only in the
`release` environment):

```sh
export GNUPGHOME="$(mktemp -d)"
gpg --batch --passphrase '' --quick-gen-key \
  'toppingctl apt repository <35013508+ktoulgaridis@users.noreply.github.com>' ed25519 sign 3y
fpr="$(gpg --with-colons --list-secret-keys | awk -F: '$1 == "fpr" { print $10; exit }')"
gpg --armor --export-secret-keys "$fpr" |
  gh secret set APT_GPG_PRIVATE_KEY --env release -R ktoulgaridis/toppingctl
gpg --armor --export "$fpr" > toppingctl-apt.pub.asc   # keep; also published by the workflow
gpg --armor --gen-revoke "$fpr" > toppingctl-apt.revoke.asc   # keep offline
rm -rf "$GNUPGHOME"
```

The workflow publishes the public key as `ktoulgaridis-archive-keyring.gpg` at the root
of the apt repo. The key expires in three years: extend it (`gpg --quick-set-expire`)
and update the secret before then.

### 4. The token and the `release` environment

1. Create a fine-grained token (*Settings → Developer settings → Fine-grained tokens*):
   resource owner `ktoulgaridis`, *Only select repositories* `homebrew-tap` and `apt`,
   permission *Contents: Read and write* and nothing else.
2. `gh secret set PACKAGES_TOKEN --env release -R ktoulgaridis/toppingctl`, then paste it.
3. The `release` environment (*Settings → Environments*) allows deployments from `v*`
   tags only, so nothing but a release run can read its three secrets: `AUR_SSH_KEY`,
   `PACKAGES_TOKEN`, `APT_GPG_PRIVATE_KEY`.

### 5. First run

Publish the next release (v0.1.0 predates `packages.yml` and its debs, so it isn't
published to package managers). Watch the `Packages` run: the three publish jobs, then
the three install checks.

## Rollback

- AUR: push a PKGBUILD for the previous version with a higher `pkgrel` (AUR has no
  delete for a single version).
- Homebrew: revert the commit in `ktoulgaridis/homebrew-tap`.
- apt: remove the deb from `pool/` in `ktoulgaridis/apt`, then regenerate and re-sign
  the indexes (re-run `scripts/package.sh apt-repo` with a deb, or by hand), and push.
