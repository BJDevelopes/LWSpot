#!/bin/sh
# Builds LWSpot.app and its disk image in dist/.
#
#   packaging/macos/build.sh            universal (Apple silicon and Intel)
#   packaging/macos/build.sh --native   this Mac's processor only, faster
#
# The disk image is named the way the updater looks for it in a release:
# lwspot-v<version>-macos-universal.dmg. Set CODESIGN_IDENTITY to a
# "Developer ID Application: ..." identity to sign for distribution;
# without it the app is signed ad hoc, which runs on this Mac (and on others
# after right-click > Open, since it is not notarized).
set -eu

cd "$(dirname "$0")/../.."
ROOT=$(pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
DIST="$ROOT/dist"
APP="$DIST/LWSpot.app"
export MACOSX_DEPLOYMENT_TARGET=11.0

if [ "${1:-}" = "--native" ]; then
    case "$(uname -m)" in
        arm64) TARGETS="aarch64-apple-darwin"; FLAVOUR="arm64" ;;
        *) TARGETS="x86_64-apple-darwin"; FLAVOUR="x86_64" ;;
    esac
else
    TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
    FLAVOUR="universal"
fi

echo "==> LWSpot $VERSION ($FLAVOUR)"
BINARIES=""
for target in $TARGETS; do
    rustup target add "$target" >/dev/null
    cargo build --release --locked --bin lwspot --target "$target"
    BINARIES="$BINARIES target/$target/release/lwspot"
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# shellcheck disable=SC2086
lipo -create $BINARIES -output "$APP/Contents/MacOS/lwspot"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

ICONSET="$DIST/LWSpot.iconset"
rm -rf "$ICONSET"
cargo run --release --quiet --example macos_icon -- "$ICONSET"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/LWSpot.icns"
rm -rf "$ICONSET"
cp LICENSE "$APP/Contents/Resources/LICENSE.txt"

if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$APP"
else
    codesign --force --sign - "$APP"
fi
codesign --verify --strict "$APP"

DMG="$DIST/lwspot-v$VERSION-macos-$FLAVOUR.dmg"
STAGE="$DIST/dmg"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "LWSpot" -srcfolder "$STAGE" -fs HFS+ -format UDZO "$DMG"
rm -rf "$STAGE"

echo "==> $APP"
echo "==> $DMG"
