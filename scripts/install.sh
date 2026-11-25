#!/bin/sh
# Installs thinpack and its runtime template from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/0x5457/thinpack.js/main/scripts/install.sh | sh
#
# The two binaries land in the same directory because that is where the CLI looks for its
# fallback template after consulting the cache.
#
# Environment:
#   THINPACK_VERSION      version or tag to install (default: the latest release)
#   THINPACK_INSTALL_DIR  where the binaries go (default: ~/.local/bin)
#   THINPACK_BASE_URL     download location with the release assets (default: the GitHub release)
#   THINPACK_STATIC       1 forces the static Linux build; ignored on other platforms
#   THINPACK_BACKEND      quickjs (default) or v8
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

# Linux ships two archives: the smaller glibc build, and the static musl build that runs where
# the host glibc is too old or is not glibc at all. `THINPACK_STATIC=1` narrows that to the
# static build alone: it is what makes the artifacts you pack run on older distributions, at
# about 1 MiB per artifact. Which archives a release has, and the glibc floor the glibc ones
# need, is release data: `thinpack-release.toml` states it.
backend=${THINPACK_BACKEND:-quickjs}
case "${backend}" in
    quickjs | v8 | jsc) ;;
    *) fail "unknown THINPACK_BACKEND '${backend}'; expected quickjs, v8 or jsc" ;;
esac
platform=$(platform)
if [ "${backend}" = jsc ]; then
    case "${platform}" in
        *-apple-darwin) ;;
        *) fail "the JSC pair uses the macOS system JavaScriptCore framework; it is available on macOS only" ;;
    esac
fi

# The host's glibc version, or nothing when it has none: Alpine's musl reports itself, and a
# host without `ldd` reports nothing. A floor an archive needs is not this: it is release data.
host_glibc() {
    [ "$(uname -s)" = Linux ] || return 0
    banner=$(ldd --version 2>/dev/null | head -n 1)
    case "${banner}" in
        "" | *musl*) return 0 ;;
    esac
    printf '%s\n' "${banner}" | sed -n 's/.*[^0-9]\([0-9][0-9]*\)\.\([0-9][0-9]*\)$/\1.\2/p'
}
glibc=$(host_glibc)

# The glibc archive is built on Ubuntu 24.04, so 2.39 is its floor. The manifest states the
# measured floor; only a release published before it needs this constant.
needs_glibc_2_39() {
    [ -n "${glibc}" ] || return 1
    host_major=${glibc%%.*}
    host_minor=${glibc#*.}
    [ "${host_major}" -gt 2 ] || { [ "${host_major}" -eq 2 ] && [ "${host_minor}" -ge 39 ]; }
}

# Prints one record per archive: `asset <name>` for each archive to try, in the order to try
# them, and `skip <name> <reason>` for an archive the release has but this host cannot run.
select_assets() {
    if [ -n "${manifest}" ]; then
        printf '%s\n' "${manifest}" | awk -v backend="${backend}" -v platform="${platform}" \
            -v glibc="${glibc}" -v static="${THINPACK_STATIC:-}" '
            function value(line) {
                sub(/^[^=]*=[[:space:]]*"/, "", line)
                sub(/"[[:space:]]*$/, "", line)
                return line
            }
            function at_least(have, need,   h, n, i) {
                split(have, h, ".")
                split(need, n, ".")
                for (i = 1; i <= 3; i++) {
                    h[i] += 0
                    n[i] += 0
                    if (h[i] != n[i]) return h[i] > n[i]
                }
                return 1
            }
            # The static build is the fallback, so runnable glibc archives come first. Every
            # table is one archive: the keys are reset so a missing `min_glibc` cannot inherit
            # the floor of the table above it.
            function finish() {
                if (archive == "") return
                if (backend_name == backend &&
                    (target == platform || index(target, platform "-") == 1)) {
                    if (min_glibc == "") {
                        # No floor: the static Linux build, or the only archive for a
                        # non-Linux target. `THINPACK_STATIC=1` rules out the glibc build, not
                        # these.
                        fallback[++fallbacks] = archive
                    } else if (static == "1") {
                        skipped[++skips] = archive " THINPACK_STATIC=1 asks for the static build"
                    } else if (glibc == "") {
                        skipped[++skips] = archive " the host has no glibc, and it needs " min_glibc
                    } else if (!at_least(glibc, min_glibc)) {
                        skipped[++skips] = archive " the host glibc " glibc " is older than " min_glibc
                    } else {
                        preferred[++preferred_count] = archive
                    }
                }
                archive = ""
                backend_name = ""
                target = ""
                min_glibc = ""
            }
            /^\[\[asset\]\]/ { finish(); next }
            /^archive[[:space:]]*=/ { archive = value($0); next }
            /^backend[[:space:]]*=/ { backend_name = value($0); next }
            /^target[[:space:]]*=/ { target = value($0); next }
            /^min_glibc[[:space:]]*=/ { min_glibc = value($0); next }
            END {
                finish()
                for (i = 1; i <= preferred_count; i++) print "asset " preferred[i]
                for (i = 1; i <= fallbacks; i++) print "asset " fallback[i]
                for (i = 1; i <= skips; i++) print "skip " skipped[i]
            }'
        return
    fi

    # Releases published before the manifest existed: their archive names carry a backend
    # suffix, and this script still knows their layout. Say which facts are being used, since
    # the floor is a constant here rather than a measurement.
    printf 'install: no thinpack-release.toml in this release; using the layout and glibc floor it implies\n' >&2
    case "${backend}" in
        quickjs) suffix="" ;;
        v8) suffix="-v8" ;;
        jsc) suffix="-jsc" ;;
    esac
    case "${platform}" in
        *-unknown-linux)
            if [ "${backend}" = v8 ]; then
                [ "${THINPACK_STATIC:-}" != 1 ] ||
                    fail "V8 release archives on Linux need glibc, and there is no static V8 build"
                candidates="${platform}-gnu"
            elif [ "${THINPACK_STATIC:-}" = "1" ]; then
                candidates="${platform}-musl"
            else
                candidates="${platform}-gnu ${platform}-musl"
            fi
            ;;
        *) candidates="${platform}" ;;
    esac
    for candidate in ${candidates}; do
        archive="thinpack-${version}${suffix}-${candidate}.tar.gz"
        case "${candidate}" in
            *-gnu)
                needs_glibc_2_39 || {
                    printf 'skip %s the host glibc %s is older than 2.39\n' "${archive}" "${glibc:-none}"
                    continue
                }
                ;;
        esac
        printf 'asset %s\n' "${archive}"
    done
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
# An older release has no manifest, and that is not a failure: `select_assets` describes its
# layout itself.
manifest=$(fetch "${base}/thinpack-release.toml" 2>/dev/null) || manifest=""

selection="${work}/assets"
select_assets > "${selection}"
tried=""
skipped=""
downloaded=""
while read -r action name reason; do
    case "${action}" in
        asset)
            tried="${tried} ${name}"
            # No `2>/dev/null`: the fetcher's own message is the difference between a network
            # failure and a release that has no archive for this target.
            if download "${base}/${name}" "${work}/${name}"; then
                downloaded="${name}"
                break
            fi
            ;;
        skip) skipped="${skipped} ${name} (${reason})" ;;
    esac
done < "${selection}"
[ -n "${downloaded}" ] ||
    fail "no archive this host can run in ${base} (backend ${backend}; tried${tried:- nothing}${skipped:+; skipped:${skipped}}); build from source"
staging="${downloaded%.tar.gz}"
say "downloaded ${downloaded}"

expected=$(fetch "${base}/SHA256SUMS" | grep -F "  ${downloaded}" | cut -d' ' -f1)
[ -n "${expected}" ] || fail "SHA256SUMS has no entry for ${downloaded}"
actual=$(digest "${work}/${downloaded}")
[ "${expected}" = "${actual}" ] || fail "checksum mismatch: expected ${expected}, got ${actual}"

tar -xzf "${work}/${downloaded}" -C "${work}"
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
