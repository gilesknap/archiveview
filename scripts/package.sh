#!/bin/bash
# Bundle the release binaries and supporting files into <name>.tar.gz
#
# Layout:  <name>/bin/{archiveview,archiveview-ssh,archiveview-mockarchiver}
#          <name>/README.md, LICENSE, THIRD_PARTY_LICENSES.txt, install.sh
set -euo pipefail

name=${1:?usage: package.sh <name>}
target=${TARGET_DIR:-target/release}
out=$(mktemp -d)/${name}
mkdir -p "${out}/bin"

cp "${target}/archiveview" "${target}/archiveview-mockarchiver" "${out}/bin/"
cp scripts/archiveview-ssh "${out}/bin/"
cp README.md LICENSE install.sh "${out}/"
"$(dirname "$0")/third-party-licenses.py" "${LICENSE_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}" \
    > "${out}/THIRD_PARTY_LICENSES.txt"

# owned by root in the archive; GNU tar and bsdtar (macOS) spell this differently
if tar --version | grep -q GNU; then
    owner=(--owner=0 --group=0)
else
    owner=(--uid 0 --gid 0)
fi
tar "${owner[@]}" -C "$(dirname "${out}")" -czf "${name}.tar.gz" "${name}"
echo "wrote ${name}.tar.gz"
