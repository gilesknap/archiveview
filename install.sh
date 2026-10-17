#!/bin/bash
# Install archiveview for aarch64 or x86_64 Linux or macOS into ~/.local
#
#   curl -sSfL https://raw.githubusercontent.com/gilesknap/archiveview/main/install.sh | bash
#
# Run from inside an unpacked release tarball, it installs that instead of downloading.
set -euo pipefail

repo=gilesknap/archiveview
case $(uname -m) in
    aarch64 | arm64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) echo "archiveview: no release build for $(uname -m)" >&2; exit 1 ;;
esac
case $(uname -s) in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) echo "archiveview: no release build for $(uname -s)" >&2; exit 1 ;;
esac
name=archiveview-${arch}-${os}
prefix=${PREFIX:-$HOME/.local}
here=$(cd "$(dirname "${BASH_SOURCE[0]:-.}")" && pwd)

if [[ -x "${here}/bin/archiveview" ]]; then
    src=${here}
else
    tmp=$(mktemp -d)
    trap 'rm -rf "${tmp}"' EXIT
    if command -v gh >/dev/null; then
        gh release download --repo "${repo}" -p "${name}.tar.gz" -D "${tmp}"
    else
        curl -sSfL -o "${tmp}/${name}.tar.gz" \
            "https://github.com/${repo}/releases/latest/download/${name}.tar.gz"
    fi
    tar --no-same-owner -C "${tmp}" -xzf "${tmp}/${name}.tar.gz"
    src=${tmp}/${name}
fi

mkdir -p "${prefix}/bin" "${prefix}/share/archiveview"
cp "${src}/bin/"* "${prefix}/bin/"
cp "${src}/README.md" "${src}/LICENSE" "${prefix}/share/archiveview/"
if [[ -f "${src}/THIRD_PARTY_LICENSES.txt" ]]; then
    cp "${src}/THIRD_PARTY_LICENSES.txt" "${prefix}/share/archiveview/"
fi
echo "installed archiveview, archiveview-ssh and archiveview-mockarchiver into ${prefix}/bin"
