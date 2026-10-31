#!/usr/bin/env bash
# Build a release bundle: dcontroller-<version>.tar.gz with the server binary,
# the trunk dist as web/, the systemd unit and install.sh. Prints the path.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
(cd crates/web && trunk build --release >&2)
cargo build --release -p dcontroller >&2
ver=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml | head -1)
test -n "$ver"
out="$root/target/bundle/dcontroller-$ver"
rm -rf "$out" && mkdir -p "$out"
cp target/release/dcontroller "$out/"
cp -r crates/web/dist "$out/web"
cp deploy/dcontroller.service deploy/install.sh "$out/"
tar -C "$root/target/bundle" -czf "$out.tar.gz" "dcontroller-$ver"
echo "$out.tar.gz"
