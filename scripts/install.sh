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
#   THINPACK_STATIC       1 forces the static Linux build; ignored on other platforms
set -eu

REPOSITORY="0x5457/thinpack.js"
LATEST_URL="https://github.com/${REPOSITORY}/releases/latest"
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

# The release target this host runs, before the libc suffix Linux adds below.
platform() {
    system=$(uname -s)
    machine=$(uname -m)
    case "${system}:${machine}" in
        Linux:x86_64) echo x86_64-unknown-linux ;;
        Linux:aarch64 | Linux:arm64) echo aarch64-unknown-linux ;;
        Darwin:arm64) echo aarch64-apple-darwin ;;
        Darwin:x86_64) echo x86_64-apple-darwin ;;
        *) fail "no archive for ${system} ${machine}; see the releases page, or build from source" ;;
    esac
}

# Linux ships two archives, in order of preference: the glibc build is the smaller one and comes
# first when the host's libc can run it, and the static musl build follows so a host that cannot,
# can. `THINPACK_STATIC=1` narrows that to the static build alone, with no glibc fallback: it is
# what makes the artifacts you pack run on older distributions, at about 1 MiB per artifact.
platform=$(platform)
case "${platform}" in
    *-unknown-linux)
        if [ "${THINPACK_STATIC:-}" = "1" ]; then
            targets="${platform}-musl"
        else
            targets="${platform}-gnu ${platform}-musl"
        fi
        ;;
    *) targets="${platform}" ;;
esac

# The glibc archive carries the runner's floor (Ubuntu 24.04 → 2.39). A host below it skips that
# candidate instead of being told to go away, because the static archive is right there.
runs_glibc() {
    [ "$(uname -s)" = Linux ] || return 0
    banner=$(ldd --version 2>&1 | head -n 1)
    case "${banner}" in
        *musl*) return 1 ;;
    esac
    # A distinct name: a shell function has no scope, and `${version}` is the release above.
    glibc=$(printf '%s\n' "${banner}" | sed -n 's/.*[^0-9]\([0-9][0-9]*\)\.\([0-9][0-9]*\)$/\1 \2/p')
    [ -n "${glibc}" ] || return 1
    major=${glibc%% *}
    minor=${glibc#* }
    [ "${major}" -gt 2 ] || { [ "${major}" -eq 2 ] && [ "${minor}" -ge 39 ]; }
}

# The release tag behind `/releases/latest`, read from the redirect rather than the API: the
# unauthenticated API allows 60 requests per hour per address, which one corporate NAT exhausts.
latest_tag() {
    if command -v curl >/dev/null 2>&1; then
        final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${LATEST_URL}")
    else
        final=$(wget -qO- "${LATEST_URL}" | grep -o 'releases/tag/v[0-9][0-9.]*' | head -n 1)
    fi
    printf '%s\n' "${final##*/}"
}

release_version() {
    if [ -n "${THINPACK_VERSION:-}" ]; then
        printf '%s\n' "${THINPACK_VERSION#v}"
        return
    fi
    tag=$(latest_tag)
    case "${tag}" in
        v[0-9]*) printf '%s\n' "${tag#v}" ;;
        *) fail "cannot read the latest release; set THINPACK_VERSION" ;;
    esac
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

version=$(release_version)
base=$(release_base "${version}")
work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

say "installing thinpack ${version}"
tried=""
for candidate in ${targets}; do
    case "${candidate}" in
        *-gnu) runs_glibc || continue ;;
    esac
    tried="${tried} ${candidate}"
    # No `2>/dev/null`: the fetcher's own message is the difference between a network failure
    # and a release that has no archive for this target.
    if download "${base}/thinpack-${version}-${candidate}.tar.gz" \
        "${work}/thinpack-${version}-${candidate}.tar.gz"; then
        target="${candidate}"
        break
    fi
done
[ -n "${target:-}" ] ||
    fail "no archive this host can run at ${base} (tried${tried:- nothing}); build from source"
asset="thinpack-${version}-${target}.tar.gz"
staging="thinpack-${version}-${target}"
say "downloaded ${asset}"

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
