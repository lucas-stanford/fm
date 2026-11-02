#!/usr/bin/env sh
# Installs an fm release binary on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/lucas-stanford/fm/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --dir ~/bin --version v0.1.0 --no-modify-path
#
# Adds the install directory to PATH in your shell profile, once. It cannot
# change the PATH of the shell that is running it.

set -eu

VERSION="${FM_VERSION:-latest}"
REPO="${FM_REPO:-lucas-stanford/fm}"
DIR="${FM_DIR:-$HOME/.local/bin}"
MODIFY_PATH=1
[ "${FM_NO_MODIFY_PATH:-0}" != "0" ] && MODIFY_PATH=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) DIR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --no-modify-path) MODIFY_PATH=0; shift ;;
    -h|--help)
      echo "usage: install.sh [--dir DIR] [--version TAG] [--no-modify-path]"
      exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

say() { printf '  %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }

need curl
need tar

# --- resolve the target ------------------------------------------------------

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-gnu" ;;
  *) die "unsupported OS: $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
target="${arch}-${os}"

echo "fm installer"
say "release: $VERSION"
say "target:  $target"

if [ "$VERSION" = "latest" ]; then
  api="https://api.github.com/repos/$REPO/releases/latest"
else
  api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
fi

url="$(curl -fsSL "$api" \
  | grep -o "https://[^\"]*${target}[^\"]*\.tar\.gz" \
  | head -n 1)"
[ -n "$url" ] || die "no $target asset in that release"

# --- download and unpack -----------------------------------------------------

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

say "downloading $(basename "$url")"
curl -fsSL "$url" -o "$staging/fm.tar.gz"
tar -xzf "$staging/fm.tar.gz" -C "$staging"

binary="$(find "$staging" -type f -name fm -perm -u+x | head -n 1)"
[ -n "$binary" ] || die "the archive contains no fm binary"

mkdir -p "$DIR"
destination="$DIR/fm"
install -m 0755 "$binary" "$destination" 2>/dev/null || {
  cp "$binary" "$destination"
  chmod 0755 "$destination"
}
say "installed to $destination"

# --- PATH --------------------------------------------------------------------

profile_for_shell() {
  case "$(basename "${SHELL:-/bin/sh}")" in
    zsh)  printf '%s\n' "${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash)
      # macOS bash reads .bash_profile for login shells; Linux reads .bashrc.
      if [ "$(uname -s)" = "Darwin" ] && [ -f "$HOME/.bash_profile" ]; then
        printf '%s\n' "$HOME/.bash_profile"
      else
        printf '%s\n' "$HOME/.bashrc"
      fi ;;
    fish) printf '%s\n' "$HOME/.config/fish/config.fish" ;;
    *) printf '%s\n' "$HOME/.profile" ;;
  esac
}

if [ "$MODIFY_PATH" -eq 0 ]; then
  say "PATH not modified (as requested)"
else
  profile="$(profile_for_shell)"
  marker="# added by the fm installer"
  case "$profile" in
    */config.fish) line="fish_add_path \"$DIR\"" ;;
    *) line="export PATH=\"$DIR:\$PATH\"" ;;
  esac

  mkdir -p "$(dirname "$profile")"
  touch "$profile"
  # Append the line only if it is not already there; never rewrite the profile.
  if grep -Fqx "$line" "$profile"; then
    say "PATH already set up in $profile"
  else
    printf '\n%s\n%s\n' "$marker" "$line" >> "$profile"
    say "added $DIR to PATH in $profile"
  fi
fi

# --- verification ------------------------------------------------------------

PATH="$DIR:$PATH"
export PATH

echo
say "$("$destination" --version)"

resolved="$(command -v fm || true)"
if [ -n "$resolved" ] && [ "$resolved" != "$destination" ]; then
  # Compare real paths, so a symlinked bin dir is not reported as a conflict.
  real_resolved="$(cd "$(dirname "$resolved")" && pwd -P)/$(basename "$resolved")"
  real_destination="$(cd "$(dirname "$destination")" && pwd -P)/$(basename "$destination")"
  if [ "$real_resolved" != "$real_destination" ]; then
    printf 'warning: another fm earlier on PATH will run instead: %s\n' "$resolved" >&2
    printf 'warning: remove it, or put %s first.\n' "$DIR" >&2
  fi
fi

echo
echo "Open a new terminal, or activate it in this one with:"
echo "  export PATH=\"$DIR:\$PATH\""
echo
echo "Then:  fm render --help"
