#!/usr/bin/env bash
#
# br (beads_rust) installer - Bulletproof multi-platform installer
#
# One-liner install:
#   curl -fsSL "https://raw.githubusercontent.com/Dicklesworthstone/beads_rust/main/install.sh?$(date +%s)" | bash
#
# Options:
#   --version vX.Y.Z   Install specific version (default: latest)
#   --dest DIR         Install to DIR (default: ~/.local/bin)
#   --system           Install to /usr/local/bin (requires sudo)
#   --easy-mode        Auto-update PATH in shell rc files
#   --verify           Run self-test after install
#   --from-source      Build from source instead of downloading binary
#   --quiet            Suppress non-error output
#   --no-gum           Disable gum formatting even if available
#   --uninstall        Remove br and clean up
#
set -euo pipefail
umask 022
shopt -s lastpipe 2>/dev/null || true

# ============================================================================
# Configuration
# ============================================================================
VERSION="${VERSION:-}"
OWNER="${OWNER:-Dicklesworthstone}"
REPO="${REPO:-beads_rust}"
BINARY_NAME="br"
DEST_DEFAULT="$HOME/.local/bin"
DEST="${DEST:-$DEST_DEFAULT}"
EASY=0
QUIET=0
VERIFY=0
FROM_SOURCE=0
UNINSTALL=0
CHECKSUM="${CHECKSUM:-}"
CHECKSUM_URL="${CHECKSUM_URL:-}"
ARTIFACT_URL="${ARTIFACT_URL:-}"
LOCK_FILE="/tmp/br-install.lock"
SYSTEM=0
NO_GUM=0
MAX_RETRIES=3
DOWNLOAD_TIMEOUT=120

# Detect gum for fancy output (https://github.com/charmbracelet/gum)
HAS_GUM=0
if command -v gum &>/dev/null && [ -t 1 ]; then
  HAS_GUM=1
fi

# ============================================================================
# Logging with optional gum formatting
# ============================================================================
log() { [ "$QUIET" -eq 1 ] && return 0; echo -e "$@"; }

info() {
  [ "$QUIET" -eq 1 ] && return 0
  if [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ]; then
    gum style --foreground 39 "→ $*"
  else
    echo -e "\033[0;34m→\033[0m $*"
  fi
}

ok() {
  [ "$QUIET" -eq 1 ] && return 0
  if [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ]; then
    gum style --foreground 42 "✓ $*"
  else
    echo -e "\033[0;32m✓\033[0m $*"
  fi
}

warn() {
  if [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ]; then
    gum style --foreground 214 "⚠ $*"
  else
    echo -e "\033[1;33m⚠\033[0m $*" >&2
  fi
}

err() {
  if [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ]; then
    gum style --foreground 196 "✗ $*"
  else
    echo -e "\033[0;31m✗\033[0m $*" >&2
  fi
}

die() {
  err "$@"
  exit 1
}

# Spinner wrapper for long operations
run_with_spinner() {
  local title="$1"
  shift
  if [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ] && [ "$QUIET" -eq 0 ]; then
    gum spin --spinner dot --title "$title" -- "$@"
  else
    info "$title"
    "$@"
  fi
}

# ============================================================================
# Usage
# ============================================================================
usage() {
  cat <<'EOF'
br installer - Install beads_rust (br) CLI tool

Usage:
  curl -fsSL https://raw.githubusercontent.com/Dicklesworthstone/beads_rust/main/install.sh | bash
  curl -fsSL .../install.sh | bash -s -- [OPTIONS]

Options:
  --version vX.Y.Z   Install specific version (default: latest)
  --dest DIR         Install to DIR (default: ~/.local/bin)
  --system           Install to /usr/local/bin (requires sudo)
  --easy-mode        Auto-update PATH in shell rc files
  --verify           Run self-test after install
  --from-source      Build from source instead of downloading binary
  --quiet            Suppress non-error output
  --no-gum           Disable gum formatting even if available
  --uninstall        Remove br and clean up

Environment Variables:
  HTTPS_PROXY        Use HTTP proxy for downloads
  BR_INSTALL_DIR     Override default install directory
  VERSION            Override version to install

Examples:
  # Default install
  curl -fsSL .../install.sh | bash

  # Custom prefix with easy mode
  curl -fsSL .../install.sh | bash -s -- --dest=/usr/local/bin --easy-mode

  # Force source build
  curl -fsSL .../install.sh | bash -s -- --from-source

  # Uninstall
  curl -fsSL .../install.sh | bash -s -- --uninstall
EOF
  exit 0
}

# ============================================================================
# Argument Parsing
# ============================================================================
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2;;
    --version=*) VERSION="${1#*=}"; shift;;
    --dest) DEST="$2"; shift 2;;
    --dest=*) DEST="${1#*=}"; shift;;
    --system) SYSTEM=1; DEST="/usr/local/bin"; shift;;
    --easy-mode) EASY=1; shift;;
    --verify) VERIFY=1; shift;;
    --artifact-url) ARTIFACT_URL="$2"; shift 2;;
    --checksum) CHECKSUM="$2"; shift 2;;
    --checksum-url) CHECKSUM_URL="$2"; shift 2;;
    --from-source) FROM_SOURCE=1; shift;;
    --quiet|-q) QUIET=1; shift;;
    --no-gum) NO_GUM=1; shift;;
    --uninstall) UNINSTALL=1; shift;;
    -h|--help) usage;;
    *) shift;;
  esac
done

# Environment variable overrides
[ -n "${BR_INSTALL_DIR:-}" ] && DEST="$BR_INSTALL_DIR"

# ============================================================================
# Uninstall
# ============================================================================
do_uninstall() {
  info "Uninstalling br..."

  if [ -f "$DEST/$BINARY_NAME" ]; then
    rm -f "$DEST/$BINARY_NAME"
    ok "Removed $DEST/$BINARY_NAME"
  else
    warn "Binary not found at $DEST/$BINARY_NAME"
  fi

  # Remove PATH modifications from shell rc files
  for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile" "$HOME/.config/fish/config.fish"; do
    if [ -f "$rc" ] && grep -q "# br installer" "$rc" 2>/dev/null; then
      if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' '/# br installer/d' "$rc" 2>/dev/null || true
      else
        sed -i '/# br installer/d' "$rc" 2>/dev/null || true
      fi
      info "Cleaned $rc"
    fi
  done

  ok "br uninstalled successfully"
  exit 0
}

[ "$UNINSTALL" -eq 1 ] && do_uninstall

# ============================================================================
# Show fancy header
# ============================================================================
if [ "$QUIET" -eq 0 ]; then
  if [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ]; then
    gum style \
      --border normal \
      --border-foreground 39 \
      --padding "0 1" \
      --margin "1 0" \
      "$(gum style --foreground 42 --bold 'br installer')" \
      "$(gum style --foreground 245 'Agent-first issue tracker')"
  else
    echo ""
    echo -e "\033[1;32mbr installer\033[0m"
    echo -e "\033[0;90mAgent-first issue tracker (beads_rust)\033[0m"
    echo ""
  fi
fi

# ============================================================================
# Platform Detection
# ============================================================================
detect_platform() {
  local os arch

  case "$(uname -s)" in
    Linux*)  os="linux" ;;
    Darwin*) os="darwin" ;;
    MINGW*|MSYS*|CYGWIN*) os="windows" ;;
    *) die "Unsupported OS: $(uname -s)" ;;
  esac

  case "$(uname -m)" in
    x86_64|amd64) arch="amd64" ;;
    aarch64|arm64) arch="arm64" ;;
    armv7*) arch="armv7" ;;
    *) die "Unsupported architecture: $(uname -m)" ;;
  esac

  echo "${os}_${arch}"
}

# ============================================================================
# Version Resolution
# ============================================================================
resolve_version() {
  if [ -n "$VERSION" ]; then return 0; fi

  info "Resolving latest version..."
  local latest_url="https://api.github.com/repos/${OWNER}/${REPO}/releases/latest"
  local tag=""

  # Try GitHub API first
  if command -v curl &>/dev/null; then
    tag=$(curl -fsSL -H "Accept: application/vnd.github.v3+json" "$latest_url" 2>/dev/null | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || echo "")
  elif command -v wget &>/dev/null; then
    tag=$(wget -qO- "$latest_url" 2>/dev/null | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || echo "")
  fi

  if [ -n "$tag" ] && [[ "$tag" =~ ^v[0-9] ]]; then
    VERSION="$tag"
    info "Latest version: $VERSION"
    return 0
  fi

  # Fallback: try redirect-based resolution
  local redirect_url="https://github.com/${OWNER}/${REPO}/releases/latest"
  if command -v curl &>/dev/null; then
    tag=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$redirect_url" 2>/dev/null | sed -E 's|.*/tag/||' || echo "")
  fi

  if [ -n "$tag" ] && [[ "$tag" =~ ^v[0-9] ]] && [[ "$tag" != *"/"* ]]; then
    VERSION="$tag"
    info "Latest version (via redirect): $VERSION"
    return 0
  fi

  warn "Could not resolve latest version; will try building from source"
  VERSION=""
}

# ============================================================================
# Cross-platform locking using mkdir (atomic on all POSIX systems)
# ============================================================================
LOCK_DIR="${LOCK_FILE}.d"
LOCKED=0

acquire_lock() {
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    LOCKED=1
    echo $$ > "$LOCK_DIR/pid"
    return 0
  fi

  # Check if existing lock is stale
  if [ -f "$LOCK_DIR/pid" ]; then
    local old_pid
    old_pid=$(cat "$LOCK_DIR/pid" 2>/dev/null || echo "")

    # Check if process is still running
    if [ -n "$old_pid" ] && ! kill -0 "$old_pid" 2>/dev/null; then
      warn "Removing stale lock (PID $old_pid not running)"
      rm -rf "$LOCK_DIR"
      if mkdir "$LOCK_DIR" 2>/dev/null; then
        LOCKED=1
        echo $$ > "$LOCK_DIR/pid"
        return 0
      fi
    fi

    # Check lock age (5 minute timeout)
    local lock_age=0
    if [[ "$OSTYPE" == "darwin"* ]]; then
      lock_age=$(( $(date +%s) - $(stat -f %m "$LOCK_DIR/pid" 2>/dev/null || echo 0) ))
    else
      lock_age=$(( $(date +%s) - $(stat -c %Y "$LOCK_DIR/pid" 2>/dev/null || echo 0) ))
    fi

    if [ "$lock_age" -gt 300 ]; then
      warn "Removing stale lock (age: ${lock_age}s)"
      rm -rf "$LOCK_DIR"
      if mkdir "$LOCK_DIR" 2>/dev/null; then
        LOCKED=1
        echo $$ > "$LOCK_DIR/pid"
        return 0
      fi
    fi
  fi

  if [ "$LOCKED" -eq 0 ]; then
    die "Another installation is running. If incorrect, run: rm -rf $LOCK_DIR"
  fi
}

# ============================================================================
# Cleanup
# ============================================================================
TMP=""
cleanup() {
  [ -n "$TMP" ] && rm -rf "$TMP"
  [ "$LOCKED" -eq 1 ] && rm -rf "$LOCK_DIR"
}
trap cleanup EXIT

# ============================================================================
# PATH modification
# ============================================================================
maybe_add_path() {
  case ":$PATH:" in
    *:"$DEST":*) return 0;;
    *)
      if [ "$EASY" -eq 1 ]; then
        local updated=0
        for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
          if [ -f "$rc" ] && [ -w "$rc" ]; then
            if ! grep -qF "$DEST" "$rc" 2>/dev/null; then
              echo "" >> "$rc"
              echo "export PATH=\"$DEST:\$PATH\"  # br installer" >> "$rc"
            fi
            updated=1
          fi
        done

        # Handle fish shell
        local fish_config="$HOME/.config/fish/config.fish"
        if [ -f "$fish_config" ] && [ -w "$fish_config" ]; then
          if ! grep -qF "$DEST" "$fish_config" 2>/dev/null; then
            echo "" >> "$fish_config"
            echo "set -gx PATH $DEST \$PATH  # br installer" >> "$fish_config"
          fi
          updated=1
        fi

        if [ "$updated" -eq 1 ]; then
          warn "PATH updated; restart shell or run: export PATH=\"$DEST:\$PATH\""
        else
          warn "Add $DEST to PATH to use br"
        fi
      else
        warn "Add $DEST to PATH to use br"
      fi
    ;;
  esac
}

# ============================================================================
# Fix shell alias conflicts
# ============================================================================
fix_alias_conflicts() {
  # Check if 'br' is aliased to something else (common: bun run)
  for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
    if [ -f "$rc" ]; then
      # Add unalias after any potential alias definitions
      if ! grep -q "unalias br.*# br installer" "$rc" 2>/dev/null; then
        if grep -q "alias br=" "$rc" 2>/dev/null || grep -q "\.bun" "$rc" 2>/dev/null; then
          echo "" >> "$rc"
          echo "unalias br 2>/dev/null  # br installer - remove conflicting alias" >> "$rc"
          info "Added unalias to $rc to prevent conflicts"
        fi
      fi
    fi
  done
}

# ============================================================================
# Rust installation for source builds
# ============================================================================
ensure_rust() {
  if [ "${RUSTUP_INIT_SKIP:-0}" != "0" ]; then
    info "Skipping rustup (RUSTUP_INIT_SKIP set)"
    return 0
  fi

  if command -v cargo >/dev/null 2>&1; then
    return 0
  fi

  if [ "$EASY" -ne 1 ] && [ -t 0 ]; then
    echo -n "Rust not found. Install via rustup? (Y/n): "
    read -r ans
    case "$ans" in n|N) warn "Skipping rustup"; return 1;; esac
  fi

  info "Installing Rust via rustup..."
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
  export PATH="$HOME/.cargo/bin:$PATH"

  # Source cargo env
  [ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
}

# ============================================================================
# Pre-build cleanup for source builds
# ============================================================================
prepare_for_build() {
  # Kill any stuck cargo processes
  pkill -9 -f "cargo build" 2>/dev/null || true

  # Clear cargo locks
  rm -f ~/.cargo/.package-cache 2>/dev/null || true
  rm -f ~/.cargo/registry/.crate-cache.lock 2>/dev/null || true

  # Clean up old br build directories
  rm -rf /tmp/br-build-* 2>/dev/null || true

  # Check disk space (need at least 1GB)
  local avail_kb
  if [[ "$OSTYPE" == "darwin"* ]]; then
    avail_kb=$(df -k /tmp | tail -1 | awk '{print $4}')
  else
    avail_kb=$(df -k /tmp | tail -1 | awk '{print $4}')
  fi

  if [ "$avail_kb" -lt 1048576 ]; then
    warn "Low disk space in /tmp ($(( avail_kb / 1024 ))MB). Cleaning up..."
    # Clean cargo target directories
    rm -rf /tmp/cargo-target 2>/dev/null || true
    rm -rf ~/.cargo/registry/cache 2>/dev/null || true
  fi

  sleep 1
}

# ============================================================================
# Download with retry
# ============================================================================
download_file() {
  local url="$1"
  local dest="$2"
  local attempt=0

  while [ $attempt -lt $MAX_RETRIES ]; do
    attempt=$((attempt + 1))

    if command -v curl &>/dev/null; then
      if curl -fsSL \
          ${HTTPS_PROXY:+--proxy "$HTTPS_PROXY"} \
          --max-time "$DOWNLOAD_TIMEOUT" \
          --retry 2 \
          -o "$dest" \
          "$url" 2>/dev/null; then
        return 0
      fi
    elif command -v wget &>/dev/null; then
      if wget -q \
          ${HTTPS_PROXY:+--proxy "$HTTPS_PROXY"} \
          --timeout="$DOWNLOAD_TIMEOUT" \
          -O "$dest" \
          "$url" 2>/dev/null; then
        return 0
      fi
    else
      die "Neither curl nor wget found"
    fi

    [ $attempt -lt $MAX_RETRIES ] && sleep 2
  done

  return 1
}

# ============================================================================
# Build from source
# ============================================================================
build_from_source() {
  info "Building from source..."

  if ! ensure_rust; then
    die "Rust is required for source builds"
  fi

  prepare_for_build

  local build_dir="$TMP/src"

  info "Cloning repository..."
  if ! git clone --depth 1 "https://github.com/${OWNER}/${REPO}.git" "$build_dir" 2>/dev/null; then
    die "Failed to clone repository"
  fi

  info "Building with Cargo (this may take a few minutes)..."

  # Build with explicit target dir to avoid conflicts
  local target_dir="$TMP/target"
  if ! (cd "$build_dir" && CARGO_TARGET_DIR="$target_dir" cargo build --release 2>&1); then
    die "Build failed"
  fi

  # Find the binary
  local bin="$target_dir/release/$BINARY_NAME"
  if [ ! -x "$bin" ]; then
    # Try to find it
    bin=$(find "$target_dir" -name "$BINARY_NAME" -type f -perm -111 2>/dev/null | head -1)
  fi

  if [ ! -x "$bin" ]; then
    die "Binary not found after build"
  fi

  install -m 0755 "$bin" "$DEST/$BINARY_NAME"
  ok "Installed to $DEST/$BINARY_NAME (source build)"
}

# ============================================================================
# Download release binary
# ============================================================================
download_release() {
  local platform="$1"

  # Map platform to release asset name
  local archive_name="br-${VERSION}-${platform}.tar.gz"
  local url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${archive_name}"

  info "Downloading $archive_name..."

  if ! download_file "$url" "$TMP/$archive_name"; then
    return 1
  fi

  # Download and verify checksum
  local checksum_url=""
  if [ -n "$CHECKSUM_URL" ]; then
    checksum_url="$CHECKSUM_URL"
  else
    checksum_url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${archive_name}.sha256"
  fi

  info "Verifying checksum..."
  if download_file "$checksum_url" "$TMP/checksum.sha256"; then
    local expected actual
    expected=$(awk '{print $1}' "$TMP/checksum.sha256")

    if command -v sha256sum &>/dev/null; then
      actual=$(sha256sum "$TMP/$archive_name" | awk '{print $1}')
    elif command -v shasum &>/dev/null; then
      actual=$(shasum -a 256 "$TMP/$archive_name" | awk '{print $1}')
    else
      warn "No SHA256 tool found, skipping verification"
      actual="$expected"
    fi

    if [ "$expected" != "$actual" ]; then
      err "Checksum mismatch!"
      err "  Expected: $expected"
      err "  Got:      $actual"
      return 1
    fi
    ok "Checksum verified"
  else
    warn "Checksum not available, skipping verification"
  fi

  # Extract
  info "Extracting..."
  if ! tar -xzf "$TMP/$archive_name" -C "$TMP" 2>/dev/null; then
    return 1
  fi

  # Find binary
  local bin="$TMP/$BINARY_NAME"
  if [ ! -x "$bin" ]; then
    bin=$(find "$TMP" -name "$BINARY_NAME" -type f -perm -111 2>/dev/null | head -1)
  fi

  if [ ! -x "$bin" ]; then
    return 1
  fi

  install -m 0755 "$bin" "$DEST/$BINARY_NAME"
  ok "Installed to $DEST/$BINARY_NAME"
  return 0
}

# ============================================================================
# Main
# ============================================================================
main() {
  acquire_lock

  TMP=$(mktemp -d)

  local platform
  platform=$(detect_platform)
  info "Platform: $platform"
  info "Install directory: $DEST"

  mkdir -p "$DEST"

  # Try binary download first (unless --from-source)
  if [ "$FROM_SOURCE" -eq 0 ]; then
    resolve_version

    if [ -n "$VERSION" ]; then
      if download_release "$platform"; then
        # Success - continue to post-install
        :
      else
        warn "Binary download failed, building from source..."
        build_from_source
      fi
    else
      warn "No release version found, building from source..."
      build_from_source
    fi
  else
    build_from_source
  fi

  # Post-install steps
  maybe_add_path
  fix_alias_conflicts

  # Verify installation
  if [ "$VERIFY" -eq 1 ]; then
    "$DEST/$BINARY_NAME" --version || true
    ok "Self-test complete"
  fi

  echo ""
  ok "br installed successfully!"
  echo ""

  # Try to get version
  local installed_version
  installed_version=$("$DEST/$BINARY_NAME" --version 2>/dev/null || echo "unknown")
  echo "  Version:  $installed_version"
  echo "  Location: $DEST/$BINARY_NAME"
  echo ""

  if [[ ":$PATH:" != *":$DEST:"* ]]; then
    echo "  To use br, restart your shell or run:"
    echo "    export PATH=\"$DEST:\$PATH\""
    echo ""
  fi

  echo "  Get started:"
  echo "    br init            # Initialize a workspace"
  echo "    br create          # Create an issue"
  echo "    br list            # List issues"
  echo "    br --help          # Full help"
  echo ""
}

# Run main - handles both direct execution and piped input (curl | bash)
main "$@"
