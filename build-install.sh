#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"

# Pull latest if this is an update
if [ -d .git ]; then
    git pull --ff-only 2>/dev/null || true
fi

# Clean stale build artifacts
rm -f br ~/prj/util/bin/br
cargo clean 2>/dev/null || true

# Build (release mode with LTO for small binary)
cargo build --release

# Install binary
mkdir -p ~/prj/util/bin
cp target/release/br ~/prj/util/bin/br
chmod +x ~/prj/util/bin/br

echo ""
echo "========================================"
echo "INSTALLED: $(~/prj/util/bin/br --version 2>/dev/null || echo 'br')"
echo "Binary: ~/prj/util/bin/br"
echo "========================================"
