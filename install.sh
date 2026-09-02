#!/usr/bin/env bash
# webai installer — downloads the prebuilt binary for your platform from the
# latest GitHub release and prints the Chrome extension setup steps.
#
#   curl -fsSL https://raw.githubusercontent.com/<owner>/webai/main/install.sh | bash
#
set -euo pipefail

REPO="${WEBAI_REPO:-wajipu/webai}"
VERSION="${WEBAI_VERSION:-latest}"
DEST="${WEBAI_DEST:-$HOME/.webai/bin}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os-$arch" in
  Darwin-arm64) asset="webai-darwin-arm64" ;;
  Darwin-x86_64|Darwin-amd64) asset="webai-darwin-x64" ;;
  Linux-x86_64|Linux-amd64) asset="webai-linux-x64" ;;
  *)
    echo "unsupported platform: $os-$arch (see GitHub Actions matrix)"
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

mkdir -p "$DEST"
echo "downloading $asset from $url"
curl -fsSL "$url" -o "$DEST/webai"
chmod +x "$DEST/webai"

echo
echo "installed: $DEST/webai ($("$DEST/webai" --version))"
echo "add to PATH:  export PATH=\"$DEST:\$PATH\""
echo
echo "next steps:"
echo "  1. cargo-free run:  $DEST/webai serve"
echo "  2. open chrome://extensions → developer mode → load unpacked → the 'extension' folder from the release zip (webai-extension.zip)"
echo "  3. log into chatgpt.com in that browser once"
echo "  4. ask:  $DEST/webai ask \"hello\""