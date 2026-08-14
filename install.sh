#!/usr/bin/env bash
# Install or update the Pipelines plugin with a checksum-verified prebuilt
# helper from a stable GitHub release.
#
# Refuses drafts, prereleases, a tag that does not look like vX.Y.Z, a release
# whose assets are missing, and a checksum that does not match. A helper that
# runs on your machine with your GitHub token is not something to install on
# trust.
set -euo pipefail

readonly plugin_id="oma.pipelines"
readonly repository="alexandre-vl/omarchy-pipelines"
readonly repository_url="https://github.com/${repository}.git"
readonly plugins_dir="${HOME}/.config/omarchy/plugins"
readonly plugin_dir="${plugins_dir}/${plugin_id}"

fail() {
  echo "pipelines-install: $*" >&2
  exit 1
}

for command_name in curl git jq omarchy sha256sum install; do
  command -v "$command_name" >/dev/null 2>&1 || fail "required command not found: $command_name"
done

[[ $(uname -s) == "Linux" ]] || fail "prebuilt helpers are supported only on Linux"
case "$(uname -m)" in
  x86_64 | amd64) platform="linux-x86_64" ;;
  *) fail "no prebuilt helper for architecture: $(uname -m) — use ./build.sh instead" ;;
esac

(($# <= 1)) || fail "usage: ./install.sh [vX.Y.Z]"

requested_tag="${1:-}"
if [[ -n $requested_tag ]]; then
  [[ $requested_tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "release must look like v1.2.3"
  release_url="https://api.github.com/repos/${repository}/releases/tags/${requested_tag}"
else
  release_url="https://api.github.com/repos/${repository}/releases/latest"
fi

release_json=$(curl --fail --location --silent --show-error "$release_url") ||
  fail "could not resolve the requested GitHub release"

tag=$(jq -r '.tag_name // empty' <<<"$release_json")
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "GitHub did not return a stable release"
[[ -z $requested_tag || $tag == "$requested_tag" ]] ||
  fail "GitHub returned release $tag instead of $requested_tag"
jq -e '.draft == false' >/dev/null <<<"$release_json" || fail "refusing to install a draft release"
jq -e '.prerelease == false' >/dev/null <<<"$release_json" || fail "refusing to install a prerelease"

asset_name="omarchy-pipelines-helper-${tag}-${platform}"
checksum_name="${asset_name}.sha256"
asset_url=$(jq -r --arg n "$asset_name" '.assets[] | select(.name == $n) | .browser_download_url' <<<"$release_json")
checksum_url=$(jq -r --arg n "$checksum_name" '.assets[] | select(.name == $n) | .browser_download_url' <<<"$release_json")
[[ -n $asset_url && -n $checksum_url ]] ||
  fail "release $tag has no $platform helper and checksum"

if [[ ! -d $plugin_dir ]]; then
  omarchy plugin add "$repository_url" --yes
fi
[[ -d $plugin_dir/.git ]] || fail "$plugin_dir is not a git-managed Pipelines installation"

origin_url=$(git -C "$plugin_dir" remote get-url origin)
case "$origin_url" in
  https://github.com/alexandre-vl/omarchy-pipelines | https://github.com/alexandre-vl/omarchy-pipelines.git | git@github.com:alexandre-vl/omarchy-pipelines.git) ;;
  *) fail "installed plugin has an unexpected origin: $origin_url" ;;
esac

git -C "$plugin_dir" fetch --tags --quiet origin
git -C "$plugin_dir" checkout --quiet "$tag" ||
  fail "could not check out $tag — commit or stash local changes in $plugin_dir first"

workdir=$(mktemp -d)
# shellcheck disable=SC2064  # expand workdir now: it is what we want removed.
trap "rm -rf '$workdir'" EXIT

curl --fail --location --silent --show-error --output "$workdir/$asset_name" "$asset_url" ||
  fail "could not download the helper"
curl --fail --location --silent --show-error --output "$workdir/$checksum_name" "$checksum_url" ||
  fail "could not download the checksum"

expected=$(awk '{print $1}' "$workdir/$checksum_name")
actual=$(sha256sum "$workdir/$asset_name" | awk '{print $1}')
[[ -n $expected && $expected == "$actual" ]] ||
  fail "checksum mismatch for $asset_name — refusing to install"

install -Dm755 "$workdir/$asset_name" "$plugin_dir/bin/omarchy-pipelines-helper"

installed_version=$("$plugin_dir/bin/omarchy-pipelines-helper" --version 2>/dev/null || true)
[[ $installed_version == "omarchy-pipelines-helper ${tag#v}" ]] ||
  fail "installed helper reports '$installed_version', expected version ${tag#v}"

echo "pipelines-install: installed $tag"
echo "pipelines-install: add 'Pipelines' to your bar, then restart the shell with 'omarchy restart shell'"
