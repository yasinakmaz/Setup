#!/usr/bin/env bash
# Packages the crates a generated installer needs to compile against
# (inst-runtime and its dependency closure, plus inst-runtime-ui for the
# GUI) into a standalone, self-contained Cargo workspace at $1.
#
# `inst-builder` locates this tree at run time via `RuntimeLocation::discover`
# (crates/builder/src/toolchain.rs): the `INST_RUNTIME_SRC` environment
# variable, or a `runtime-src/` directory next to the running executable.
# Without it, Installer Studio and the CLI can validate and generate code
# but cannot compile anything — every Build fails with "runtime sources not
# found". A checkout of this repository already satisfies discovery (its
# root IS a superset of this tree), so this script only matters for
# distributing prebuilt Studio/CLI binaries outside the repository, as the
# release workflow does.
#
# Usage: scripts/package-runtime-src.sh <destination-dir>
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <destination-dir>" >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$1"

# Crates outside this set (inst-model, inst-catalog, inst-analyzer,
# inst-doctor, inst-codegen, inst-builder, inst-cli, inst-studio) are
# Studio-only: no generated installer crate depends on them.
studio_only='"crates/(model|catalog|analyzer|doctor|codegen|builder|cli|studio)"'

rm -rf "$dest"
mkdir -p "$dest/crates"
for crate in brand fsx i18n log payload wire fetch runtime runtime-ui; do
    cp -r "$root/crates/$crate" "$dest/crates/$crate"
done
cp "$root/Cargo.lock" "$dest/Cargo.lock"
cp "$root/rust-toolchain.toml" "$dest/rust-toolchain.toml"
grep -vE "$studio_only" "$root/Cargo.toml" > "$dest/Cargo.toml"
