#!/usr/bin/env bash
# Build the helper from source into bin/, where Service.qml looks for it.
set -euo pipefail
plugin_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cargo build --release --locked --manifest-path "$plugin_dir/backend/Cargo.toml"
install -Dm755 \
  "$plugin_dir/backend/target/release/omarchy-pipelines-helper" \
  "$plugin_dir/bin/omarchy-pipelines-helper"
echo "built $plugin_dir/bin/omarchy-pipelines-helper"
