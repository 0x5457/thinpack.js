#!/bin/sh
# Installs thinpack and its runtime template from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/0x5457/thinpack.js/main/scripts/install.sh | sh
#
# The two binaries land in the same directory because that is where the CLI looks for its
# template: `thinpack` resolves `thinpack-rt` next to itself before it consults the cache.
#
# Environment:
#   THINPACK_VERSION      version or tag to install (default: the latest release)
#   THINPACK_INSTALL_DIR  where the binaries go (default: ~/.local/bin)
#   THINPACK_BASE_URL     download location with the release assets (default: the GitHub release)
set -eu

REPOSITORY="0x5457/thinpack.js"
API_URL="https://api.github.com/repos/${REPOSITORY}/releases/latest"
RELEASE_URL="https://github.com/${REPOSITORY}/releases/download"
: "${HOME:?HOME is not set; set THINPACK_INSTALL_DIR to choose a directory}"
INSTALL_DIR="${THINPACK_INSTALL_DIR:-${HOME}/.local/bin}"

say() { printf '%s\n' "$@"; }
fail() {
    printf 'install: %s\n' "$@" >&2
    exit 1
}

command -v tar >/dev/null 2>&1 || fail "tar is required"
command -v install >/dev/null 2>&1 || fail "install (coreutils) is required"
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
    download() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
    download() { wget -qO "$2" "$1"; }
else
    fail "curl or wget is required"
fi

triple() {
    system=$(uname -s)
    machine=$(uname -m)
    case "${system}:${machine}" in
        Linux:x86_64) echo x86_64-unknown-linux-gnu ;;
        Linux:aarch64 | Linux:arm64) echo aarch64-unknown-linux-gnu ;;
        Darwin:arm64) echo aarch64-apple-darwin ;;
        Darwin:x86_64) echo x86_64-apple-darwin ;;
        *) fail "no archive for ${system} ${machine}; see the releases page, or build from source" ;;
    esac
}

# The Linux archives carry the runner's glibc floor (Ubuntu 24.04 → 2.39). Refusing an older
# loader here is better than installing a binary that the dynamic loader rejects or crashes on.
require_glibc() {
    [ "$(uname -s)" = Linux ] || return 0
    banner=$(ldd --version 2>&1 | head -n 1)
    case "${banner}" in
        *musl*) fail "this host is musl and the archives are glibc-linked; build from source" ;;
    esac
    version=$(printf '%s\n' "${banner}" | sed -n 's/.*[^0-9]\([0-9][0-9]*\)\.\([0-9][0-9]*\)$/\1 \2/p')
    [ -n "${version}" ] || fail "cannot read the glibc version from \`${banner}\`; build from source"
    major=${version%% *}
    minor=${version#* }
    if [ "${major}" -lt 2 ] || { [ "${major}" -eq 2 ] && [ "${minor}" -lt 39 ]; }; then
        fail "this host has glibc ${major}.${minor} and the archives need 2.39 (built on Ubuntu 24.04); build from source"
    fi
}

release_version() {
    if [ -n "${THINPACK_VERSION:-}" ]; then
        printf '%s\n' "${THINPACK_VERSION#v}"
        return
    fi
    tag=$(fetch "${API_URL}" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "${tag}" ] || fail "cannot read the latest release; set THINPACK_VERSION"
    printf '%s\n' "${tag#v}"
}

# Prints the release location: `.../download/v0.1.0` for a published release.
release_base() {
    if [ -n "${THINPACK_BASE_URL:-}" ]; then
        printf '%s\n' "${THINPACK_BASE_URL}"
    else
        printf '%s/v%s\n' "${RELEASE_URL}" "$1"
    fi
}

digest() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        fail "sha256sum or shasum is required to verify the download"
    fi
}

require_glibc
target=$(triple)
version=$(release_version)
base=$(release_base "${version}")
asset="thinpack-${version}-${target}.tar.gz"
staging="thinpack-${version}-${target}"

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

say "downloading ${asset}"
download "${base}/${asset}" "${work}/${asset}" || fail "${asset} is not at ${base}"

expected=$(fetch "${base}/SHA256SUMS" | grep -F "  ${asset}" | cut -d' ' -f1)
[ -n "${expected}" ] || fail "SHA256SUMS has no entry for ${asset}"
actual=$(digest "${work}/${asset}")
[ "${expected}" = "${actual}" ] || fail "checksum mismatch: expected ${expected}, got ${actual}"

tar -xzf "${work}/${asset}" -C "${work}"
mkdir -p "${INSTALL_DIR}"
INSTALL_DIR=$(cd "${INSTALL_DIR}" && pwd -P)
for binary in thinpack thinpack-rt; do
    install -m 0755 "${work}/${staging}/${binary}" "${INSTALL_DIR}/${binary}"
done
say "installed thinpack ${version} in ${INSTALL_DIR}"

case ":${PATH:-}:" in
    *":${INSTALL_DIR}:"*)
        say ""
        say "try it:"
        say "  printf 'console.log(\"hello\")' > hello.js && thinpack pack hello.js -o hello && ./hello"
        ;;
    *)
        say ""
        say "${INSTALL_DIR} is not in PATH; add it, then run: thinpack --help"
        ;;
esac
