#!/bin/sh
# Builds LWSpot.app and its disk image in dist/.
#
#   packaging/macos/build.sh            universal (Apple silicon and Intel)
#   packaging/macos/build.sh --native   this Mac's processor only, faster
#
# The disk image is named the way the updater looks for it in a release:
# lwspot-v<version>-macos-universal.dmg. Set CODESIGN_IDENTITY to a
# "Developer ID Application: ..." identity to sign for distribution;
# without it the app is signed ad hoc, which runs on this Mac and, on others,
# after the user allows it once in System Settings > Privacy & Security >
# Open Anyway. With APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD as well,
# the app and the disk image are notarized and stapled, so a download opens
# with a double-click.
set -eu

cd "$(dirname "$0")/../.."
ROOT=$(pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
DIST="$ROOT/dist"
APP="$DIST/LWSpot.app"
export MACOSX_DEPLOYMENT_TARGET=11.0
# Panic messages name source files by path. Map this machine's paths to
# neutral ones so a build never ships the builder's home folder or user
# name. Encoded flags, so paths with spaces stay whole.
SEP=$(printf '\037')
CARGO_DIR=${CARGO_HOME:-$HOME/.cargo}
RUSTUP_DIR=${RUSTUP_HOME:-$HOME/.rustup}
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$CARGO_DIR=/cargo$SEP--remap-path-prefix=$RUSTUP_DIR=/rustup$SEP--remap-path-prefix=$ROOT=/lwspot"

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

# Notarization, when there is a Developer ID and an Apple ID to submit with.
# The app gets its own stapled ticket, so the copy dragged to Applications
# opens offline too; the disk image is signed, notarized and stapled below.
NOTARIZE=""
if [ -n "${CODESIGN_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ]; then
    NOTARIZE=1
fi
notarize() {
    log="$DIST/notarize.log"
    xcrun notarytool submit "$1" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_APP_PASSWORD" --wait | tee "$log"
    grep -q "status: Accepted" "$log" || { echo "Notarization of $1 was not accepted"; exit 1; }
    rm -f "$log"
}
if [ -n "$NOTARIZE" ]; then
    ditto -c -k --keepParent "$APP" "$DIST/LWSpot-notarize.zip"
    notarize "$DIST/LWSpot-notarize.zip"
    rm -f "$DIST/LWSpot-notarize.zip"
    xcrun stapler staple "$APP"
fi

DMG="$DIST/lwspot-v$VERSION-macos-$FLAVOUR.dmg"
STAGE="$DIST/dmg"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
if [ -z "$NOTARIZE" ]; then
    # Not notarized: say how to get past Gatekeeper's first-launch stop.
    cp "packaging/macos/How to open LWSpot.txt" "$STAGE/"
fi
# hdiutil now and then finds the volume busy on a loaded machine; try again.
for attempt in 1 2 3; do
    hdiutil create -quiet -volname "LWSpot" -srcfolder "$STAGE" -fs HFS+ -format UDZO "$DMG" && break
    [ "$attempt" = 3 ] && exit 1
    sleep 5
done
rm -rf "$STAGE"
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp --sign "$CODESIGN_IDENTITY" "$DMG"
fi
if [ -n "$NOTARIZE" ]; then
    notarize "$DMG"
    xcrun stapler staple "$DMG"
    spctl --assess --type open --context context:primary-signature -v "$DMG"
fi

echo "==> $APP"
echo "==> $DMG"
