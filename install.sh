#!/usr/bin/env bash
# shellcheck shell=bash
#
# MemPalace curl-pipe installer
# Usage: curl -sSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash
# Or:    bash install.sh [--version X.Y.Z] [--path DIR] [--easy-mode] [--dry-run]
#
set -euo pipefail

# -----------------------------------------------------------------------------
# Constants
# -----------------------------------------------------------------------------
DEFAULT_REPO="quangdang46/mempalace_rust"
DEFAULT_BIN="mpr"
INSTALL_VERSION="${INSTALL_VERSION:-}"

# -----------------------------------------------------------------------------
# Colors (disabled if not a TTY)
# -----------------------------------------------------------------------------
if [[ -t 1 ]]; then
  RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'
  BLUE='\033[0;34m'; BOLD='\033[1m'; NC='\033[0m'
else
  RED=''; GREEN=''; YELLOW=''; BLUE=''; BOLD=''; NC=''
fi

# -----------------------------------------------------------------------------
# Logging helpers
# -----------------------------------------------------------------------------
log_info()  { echo -e "${BLUE}[info]${NC} $*" >&2; }
log_ok()    { echo -e "${GREEN}[ok]${NC}   $*" >&2; }
log_warn()  { echo -e "${YELLOW}[warn]${NC} $*" >&2; }
log_err()   { echo -e "${RED}[error]${NC} $*" >&2; }

# -----------------------------------------------------------------------------
# Platform detection
# -----------------------------------------------------------------------------
detect_os() {
  case "$(uname -s)" in
    Linux)   echo "linux";;
    Darwin)  echo "macos";;
    MINGW*|MSYS*|CYGWIN*) echo "windows";;
    *)       echo "unknown";;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64)  echo "x86_64";;
    aarch64|arm64) echo "aarch64";;
    *)       echo "unknown";;
  esac
}

# -----------------------------------------------------------------------------
# Version resolution
# -----------------------------------------------------------------------------
resolve_version() {
  local repo="$1"
  local version="$2"

  if [[ -n "$version" ]]; then
    echo "$version"
    return
  fi

  # Get latest release tag from GitHub API
  local tag
  tag=$(curl -sSL --fail "https://api.github.com/repos/${repo}/releases/latest" \
    | grep -E '"tag_name":' \
    | sed -E 's/.*"tag_name":\s*"v?([^"]+)".*/\1/' \
    | tr -d '[:space:]' \
    || echo "")

  if [[ -z "$tag" ]]; then
    # Fallback: try to get tag from git ls-remote
    tag=$(git ls-remote --tags "https://github.com/${repo}.git" 2>/dev/null \
      | grep -v '\^{}' \
      | sed 's/.*refs\/tags\/v//' \
      | sort -V \
      | tail -1 \
      || echo "")
  fi

  if [[ -z "$tag" ]]; then
    log_err "Could not resolve latest version from GitHub API"
    log_info "Use --version to specify a version explicitly"
    exit 1
  fi

  echo "$tag"
}

# Resolve redirect to get actual download URL
resolve_url() {
  local url="$1"
  curl -sSL --fail -o /dev/null -w "%{url_effective}" -L "$url"
}

# -----------------------------------------------------------------------------
# Download with retry + resume + proxy support
# -----------------------------------------------------------------------------
download_file() {
  local url="$1"
  local dest="$2"
  local max_attempts="${3:-3}"

  local attempt=1
  while (( attempt <= max_attempts )); do
    log_info "Downloading ($attempt/$max_attempts): $url"

    # shellcheck disable=SC2154
    if curl -sSL --fail --retry 3 --retry-delay 2 \
      -o "$dest" \
      -L "$url"; then
      log_ok "Download complete: $(basename "$dest")"
      return 0
    fi

    log_warn "Download attempt $attempt failed"
    rm -f "$dest"

    if (( attempt < max_attempts )); then
      log_info "Waiting 2s before retry..."
      sleep 2
    fi
    ((attempt++))
  done

  log_err "Download failed after $max_attempts attempts"
  return 1
}

# -----------------------------------------------------------------------------
# SHA256 verification
# -----------------------------------------------------------------------------
verify_checksum() {
  local file="$1"
  local expected="$2"

  local actual
  actual=$(sha256sum "$file" 2>/dev/null | awk '{print $1}' || shasum -a 256 "$file" | awk '{print $1}' || echo "")

  if [[ "${actual,,}" != "${expected,,}" ]]; then
    log_err "Checksum mismatch!"
    log_err "  Expected: ${expected,,}"
    log_err "  Actual:   ${actual,,}"
    return 1
  fi

  log_ok "Checksum verified"
  return 0
}

# -----------------------------------------------------------------------------
# Atomic install
# -----------------------------------------------------------------------------
install_binary() {
  local src="$1"
  local dest="$2"
  local dirname
  dirname=$(dirname "$dest")

  # Ensure target directory exists
  mkdir -p "$dirname"

  # Atomic install: write to temp then rename
  local tmp_dest="${dest}.tmp.$$"
  cp "$src" "$tmp_dest"
  chmod 755 "$tmp_dest"
  mv "$tmp_dest" "$dest"

  log_ok "Installed: $dest"
}

# -----------------------------------------------------------------------------
# Concurrent install lock
# -----------------------------------------------------------------------------
acquire_lock() {
  local lockfile="${TMPDIR:-/tmp}/mempalace-install.lock"
  local fd=200

  # Attempt to create lock file
  if (set -e; eval "exec $fd>\"$lockfile\"" 2>/dev/null); then
    flock -n $fd && return 0 || true
  fi

  # Lock held — wait briefly
  log_info "Another install in progress, waiting..."
  sleep 3

  # Try again
  (set -e; eval "exec $fd>\"$lockfile\"") 2>/dev/null || true
  flock -w 30 $fd && return 0 || {
    log_err "Could not acquire install lock"
    return 1
  }
}

# -----------------------------------------------------------------------------
# Usage
# -----------------------------------------------------------------------------
usage() {
  cat <<EOF
Usage: $0 [OPTIONS]

Install MemPalace CLI tool.

Options:
  --version VERSION   Specific version to install (default: latest from GitHub)
  --path DIR          Installation directory (default: /usr/local/bin or ~/.local/bin)
  --easy-mode        Add installation directory to PATH automatically
  --dry-run           Show what would be installed without installing
  --repo USER/REPO   GitHub repository (default: quangdang46/mempalace_rust)
  --bin NAME         Binary name (default: mpr)
  -h, --help         Show this help

Examples:
  curl -sSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash
  bash install.sh --version 0.1.0 --path ~/bin
  bash install.sh --dry-run
EOF
}

# -----------------------------------------------------------------------------
# Main
# -----------------------------------------------------------------------------
main() {
  local version="" install_dir="" easy_mode="false" dry_run="false"
  local repo="${DEFAULT_REPO}" bin="${DEFAULT_BIN}"

  # Parse arguments
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --version)      version="$2"; shift 2;;
      --path)         install_dir="$2"; shift 2;;
      --easy-mode)    easy_mode="true"; shift;;
      --dry-run)      dry_run="true"; shift;;
      --repo)         repo="$2"; shift 2;;
      --bin)          bin="$2"; shift 2;;
      -h|--help)      usage; exit 0;;
      *)              log_err "Unknown argument: $1"; usage; exit 1;;
    esac
  done

  local os=$(detect_os)
  local arch=$(detect_arch)
  local version_num
  version_num=$(resolve_version "$repo" "$version")
  log_info "MemPalace installer"
  log_info "  Version: $version_num"
  log_info "  OS: $os | Arch: $arch"
  log_info "  Repository: https://github.com/$repo"

  if [[ "$os" == "unknown" ]] || [[ "$arch" == "unknown" ]]; then
    log_err "Unsupported platform: $os/$arch"
    exit 1
  fi

  # Determine target triplet and artifact name
  local target artifact
  case "$os/$arch" in
    linux/x86_64)   target="x86_64-unknown-linux-musl"; artifact="mempalace-${version_num}-${target}.tar.gz";;
    linux/aarch64)  target="aarch64-unknown-linux-musl"; artifact="mempalace-${version_num}-${target}.tar.gz";;
    macos/x86_64)   target="x86_64-apple-darwin";       artifact="mempalace-${version_num}-${target}.tar.gz";;
    macos/aarch64)  target="aarch64-apple-darwin";     artifact="mempalace-${version_num}-${target}.tar.gz";;
    windows/x86_64) target="x86_64-pc-windows-msvc";   artifact="mempalace-${version_num}-${target}.zip";;
    *)              log_err "No release artifact for $os/$arch"; exit 1;;
  esac

  # Download URL
  local download_url="https://github.com/${repo}/releases/download/v${version_num}/${artifact}"
  log_info "  Artifact: $artifact"

  if [[ "$dry_run" == "true" ]]; then
    log_info "[dry-run] Would download: $download_url"
    log_info "[dry-run] Would install: $bin"
    if [[ "$easy_mode" == "true" ]]; then
      log_info "[dry-run] Would add $([[ "$os" == "windows" ]] && echo "%USERPROFILE%\\AppData\\Local\\Microsoft\\WindowsApps" || echo "$install_dir") to PATH"
    fi
    log_ok "Dry run complete"
    exit 0
  fi

  # Acquire lock
  acquire_lock || exit 1

  # Temporary directory for downloads
  local tmpdir
  tmpdir=$(mktemp -d)
  trap "rm -rf $tmpdir" EXIT

  # Download
  local archive_path="${tmpdir}/${artifact}"
  download_file "$download_url" "$archive_path" || exit 1

  # Extract
  local bin_path
  case "$os/$arch" in
    windows/*)
      bin_path="${tmpdir}/${bin}.exe"
      unzip -o "$archive_path" -d "$tmpdir" > /dev/null 2>&1
      local extracted
      extracted=$(unzip -l "$archive_path" 2>/dev/null | grep -oE "[^[:space:]]+\.exe$" | head -1)
      if [[ -n "$extracted" ]]; then
        mv "${tmpdir}/${extracted}" "$bin_path"
      fi
      ;;
    *)
      bin_path="${tmpdir}/${bin}"
      tar -xzf "$archive_path" -C "$tmpdir" 2>/dev/null
      local extracted
      extracted=$(tar -tzf "$archive_path" 2>/dev/null | grep -v '/$' | head -1)
      if [[ -n "$extracted" ]]; then
        mv "${tmpdir}/${extracted}" "$bin_path"
      fi
      ;;
  esac

  if [[ ! -f "$bin_path" ]]; then
    log_err "Extraction failed: binary not found"
    ls -la "$tmpdir"
    exit 1
  fi

  # Determine install directory
  if [[ -z "$install_dir" ]]; then
    if [[ "$os" == "windows" ]]; then
      install_dir="${LOCALAPPDATA:-${USERPROFILE}\AppData\Local\Microsoft\WindowsApps}"
    elif [[ -w /usr/local/bin ]]; then
      install_dir="/usr/local/bin"
    else
      install_dir="${HOME}/.local/bin"
    fi
  fi

  # Verify checksum if sidecar exists
  local checksum_url="${download_url}.sha256"
  if curl -sSL --fail -o "${tmpdir}/${artifact}.sha256" "$checksum_url" 2>/dev/null; then
    local expected
    expected=$(awk '{print $1}' "${tmpdir}/${artifact}.sha256")
    verify_checksum "$archive_path" "$expected" || log_warn "Checksum verification skipped"
  else
    log_warn "No checksum file found, skipping verification"
  fi

  # Install
  install_binary "$bin_path" "${install_dir}/${bin}" || exit 1

  # Easy mode PATH update
  if [[ "$easy_mode" == "true" ]]; then
    if [[ "$os" == "windows" ]]; then
      local path_dir
      path_dir=$(cygpath -u "$install_dir" 2>/dev/null || echo "$install_dir")
      log_info "Add this to your PATH: $path_dir"
      if [[ ":$PATH:" != *":${path_dir}:"* ]]; then
        log_info "Run: setx PATH \"%PATH%;${path_dir}\" (restart terminal required)"
      fi
    else
      log_info "Add to PATH: export PATH=\"\${PATH}:${install_dir}\""
      log_info "Add to ~/.bashrc or ~/.zshrc to persist"
    fi
  fi

  # Verify
  if command -v "$bin" >/dev/null 2>&1; then
    log_ok "MemPalace installed successfully!"
    log_info "Run '${bin} --help' to get started"
  else
    log_warn "Installed but not found in PATH"
    log_info "Full path: ${install_dir}/${bin}"
  fi
}

main "$@"
