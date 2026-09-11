#!/usr/bin/env bash
# Build MpdController.app and install it into /Applications.
#
# The bundle is assembled by hand: an .app is just a directory holding the
# binary, the icon and Info.plist, so a packaging tool buys nothing for an app
# that's only ever installed from source. It's ad-hoc signed, which seals the
# bundle but is not a distributable (Developer ID) signature.
#
# The mpdcontrol:// URL scheme is only registered with Launch Services
# once the app lives in /Applications and has been launched at least once.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

cargo build --release -p controller

app_name="MpdController.app"
assets="crates/controller/assets"
built_app="target/release/${app_name}"
dest="/Applications/${app_name}"

# Prints e.g. `path+file:///.../crates/controller#0.1.0`.
pkgid="$(cargo pkgid -p controller)"
version="${pkgid##*[#@]}"

rm -rf "${built_app}"
mkdir -p "${built_app}/Contents/MacOS" "${built_app}/Contents/Resources"
cp target/release/controller "${built_app}/Contents/MacOS/controller"
cp "${assets}/icon.icns" "${built_app}/Contents/Resources/icon.icns"

plist="${built_app}/Contents/Info.plist"
cp "${assets}/Info.plist" "${plist}"
plutil -replace CFBundleShortVersionString -string "${version}" "${plist}"
plutil -replace CFBundleVersion -string "$(date -u +%Y%m%d.%H%M%S)" "${plist}"

codesign --force --sign - "${built_app}"

if [ -d "${dest}" ]; then
    echo "Removing existing ${dest}"
    rm -rf "${dest}"
fi

echo "Installing ${built_app} -> ${dest}"
cp -R "${built_app}" "${dest}"

echo "Done. Launch it once from /Applications (or Spotlight) to register the mpdcontrol:// URL scheme."
