#!/bin/sh
set -eu

REPO="rdekovich/edgelord"
INSTALL_DIR="${EDGELORD_INSTALL_DIR:-$HOME/.edgelord/bin}"

main() {
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)

    case "$os" in
        linux) os="unknown-linux-gnu" ;;
        darwin) os="apple-darwin" ;;
        *) echo "error: unsupported OS: $os" >&2; exit 1 ;;
    esac

    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *) echo "error: unsupported architecture: $arch" >&2; exit 1 ;;
    esac

    target="${arch}-${os}"

    if [ -n "${EDGELORD_VERSION:-}" ]; then
        version="$EDGELORD_VERSION"
    else
        version=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | grep '"tag_name"' | sed 's/.*"v\(.*\)".*/\1/')
    fi

    url="https://github.com/$REPO/releases/download/v${version}/edgelord-${target}.tar.gz"

    case "$version" in
        ''|*[!0-9A-Za-z.+-]*) echo "error: invalid release version" >&2; exit 1 ;;
    esac
    echo "downloading edgelord v${version} for ${target}..."
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
    archive="edgelord-${target}.tar.gz"
    curl -fsSL "$url" -o "$tmpdir/$archive"
    curl -fsSL "$url.sha256" -o "$tmpdir/$archive.sha256"
    expected=$(awk 'NR == 1 {print $1}' "$tmpdir/$archive.sha256")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmpdir/$archive" | awk '{print $1}')
    else
        actual=$(shasum -a 256 "$tmpdir/$archive" | awk '{print $1}')
    fi
    [ "${#expected}" = 64 ] && [ "$expected" = "$actual" ] || {
        echo "error: release checksum mismatch" >&2; exit 1;
    }
    tar xzf "$tmpdir/$archive" -C "$tmpdir" edgelord
    mkdir -p "$INSTALL_DIR"
    install -m 755 "$tmpdir/edgelord" "$INSTALL_DIR/edgelord"
    echo "installed edgelord to $INSTALL_DIR/edgelord"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) echo "add $INSTALL_DIR to your shell's PATH" ;;
    esac

    echo "run 'edgelord init' to get started"
}

main
