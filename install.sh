#!/usr/bin/env bash
# Build MpdController.app and install it into /Applications.
#
# The mpdcontrol:// URL scheme is only registered with Launch Services
# once the app lives in /Applications and has been launched at least once.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

cargo bundle --release

app_name="MpdController.app"
built_app="target/release/bundle/osx/${app_name}"
dest="/Applications/${app_name}"

if [ ! -d "${built_app}" ]; then
    echo "error: expected bundle not found at ${built_app}" >&2
    exit 1
fi

if [ -d "${dest}" ]; then
    echo "Removing existing ${dest}"
    rm -rf "${dest}"
fi

echo "Installing ${built_app} -> ${dest}"
cp -R "${built_app}" "${dest}"

echo "Done. Launch it once from /Applications (or Spotlight) to register the mpdcontrol:// URL scheme."
