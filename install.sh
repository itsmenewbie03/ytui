#!/usr/bin/env bash
#
# ytui installer
#
# Downloads a prebuilt ytui binary from the project's GitHub Releases and
# installs it on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/itsmenewbie03/ytui/main/install.sh | bash
#
# Usage:
#   install.sh [<version>] [--prefix <dir>]
#
# Options:
#   <version>      Release tag to install, e.g. v0.1.0 (default: latest)
#   --prefix <dir> Install directory (default: $XDG_BIN_HOME, ~/.local/bin,
#                  or /usr/local/bin when run as root)
#   --help         Show this help
#
# Environment:
#   YTUI_VERSION   Release tag (same as the positional <version> argument)
#   YTUI_PREFIX    Install directory (same as --prefix)
#   YTUI_REPO      GitHub repository in <owner>/<repo> form (default: itsmenewbie03/ytui)
#
set -euo pipefail

REPO="${YTUI_REPO:-itsmenewbie03/ytui}"
VERSION="${YTUI_VERSION:-latest}"
PREFIX=""

usage() {
  local code="${1:-0}"
  sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'
  exit "$code"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      [[ $# -ge 2 ]] || { echo "error: --prefix requires a directory" >&2; usage 1; }
      PREFIX="$2"
      shift 2
      ;;
    --help | -h)
      usage
      ;;
    --*)
      echo "error: unknown option: $1" >&2
      usage 1
      ;;
    *)
      [[ -z "$VERSION" || "$VERSION" == "latest" ]] || {
        echo "error: version specified twice" >&2
        exit 1
      }
      VERSION="$1"
      shift
      ;;
  esac
done

PREFIX="${YTUI_PREFIX:-${PREFIX:-}}"

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: $1 is required but was not found on PATH" >&2
    exit 1
  fi
}

need curl
need tar
need uname

case "$(uname -s)" in
  Linux) ;;
  *)
    echo "error: ytui prebuilt binaries are only available for Linux." >&2
    echo "Build from source instead:" >&2
    echo "  git clone https://github.com/${REPO}.git && cd ytui && cargo install --path ." >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) target="x86_64" ;;
  aarch64 | arm64)
    echo "error: no prebuilt ytui binary for aarch64 yet." >&2
    echo "Build from source instead:" >&2
    echo "  git clone https://github.com/${REPO}.git && cd ytui && cargo install --path ." >&2
    exit 1
    ;;
  *)
    echo "error: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

if [[ -z "$PREFIX" ]]; then
  if [[ "$(id -u)" -eq 0 ]]; then
    PREFIX="/usr/local/bin"
  elif [[ -n "${XDG_BIN_HOME:-}" ]]; then
    PREFIX="$XDG_BIN_HOME"
  else
    PREFIX="$HOME/.local/bin"
  fi
fi

asset="ytui-linux-${target}.tar.gz"
base="https://github.com/${REPO}/releases"
if [[ "$VERSION" == "latest" ]]; then
  url="${base}/latest/download/${asset}"
else
  case "$VERSION" in
    v*) tag="$VERSION" ;;
    *) tag="v${VERSION}" ;;
  esac
  url="${base}/download/${tag}/${asset}"
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading ${asset} from ${url}"
curl -fsSL --proto '=https' --tlsv1.2 -o "${tmpdir}/${asset}" "$url"

echo "Verifying checksum"
curl -fsSL --proto '=https' --tlsv1.2 -o "${tmpdir}/${asset}.sha256" "${url}.sha256"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmpdir" && sha256sum -c "${asset}.sha256")
else
  (cd "$tmpdir" && shasum -a 256 -c "${asset}.sha256")
fi

tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"
mkdir -p "$PREFIX"
install -m 0755 "${tmpdir}/ytui" "${PREFIX}/ytui"

echo
echo "ytui installed to ${PREFIX}/ytui"

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *)
    echo
    echo "NOTE: ${PREFIX} is not on your PATH. Add it, for example:"
    case "${SHELL##*/}" in
      zsh) echo "  echo 'export PATH=\"${PREFIX}:\$PATH\"' >> ~/.zshrc" ;;
      fish) echo "  fish_add_path ${PREFIX}" ;;
      *) echo "  echo 'export PATH=\"${PREFIX}:\$PATH\"' >> ~/.bashrc" ;;
    esac
    ;;
esac

if ! command -v mpv >/dev/null 2>&1; then
  echo
  echo "WARNING: mpv was not found on PATH. ytui needs mpv to play audio."
fi

echo
echo "Done. Run 'ytui' to start listening."