#!/bin/bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This app bundle must be built on macOS." >&2
    exit 1
fi

cargo build --release --locked

mkdir -p dist
staging="$(mktemp -d "$project_dir/dist/.Browser.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
bundle="$staging/Browser.app"
mkdir -p "$bundle/Contents/MacOS"
install -m 755 target/release/minibrowser "$bundle/Contents/MacOS/Browser"
install -m 644 macos/Info.plist "$bundle/Contents/Info.plist"

plutil -lint "$bundle/Contents/Info.plist"
if [[ -n "${BROWSER_SIGNING_IDENTITY:-}" ]]; then
    # Apple must grant this managed entitlement to the signing team first.
    codesign --force --options runtime --sign "$BROWSER_SIGNING_IDENTITY" \
        --entitlements macos/Browser.entitlements "$bundle"
else
    codesign --force --sign - --timestamp=none "$bundle"
fi
codesign --verify --strict "$bundle"
ditto -c -k --sequesterRsrc --keepParent "$bundle" "$staging/Browser-Apple-Silicon.zip"

rm -rf dist/Browser.app
mv "$bundle" dist/Browser.app
mv -f "$staging/Browser-Apple-Silicon.zip" dist/Browser-Apple-Silicon.zip
echo "Built $project_dir/dist/Browser.app"
