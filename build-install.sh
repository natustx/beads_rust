#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"

# Pull latest if this is an update
if [ -d .git ]; then
    if git remote get-url upstream &>/dev/null; then
        # Fork with rebase workflow: history may be rewritten by force-push
        _BRANCH=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
        [ -z "$_BRANCH" ] && _BRANCH="main"
        git fetch origin && git reset --hard "origin/$_BRANCH"
    else
        git pull --ff-only 2>/dev/null || true
    fi
fi

# Clean stale build artifacts
rm -f br ~/prj/util/bin/br
cargo clean

# Build
cargo build --release

# Install binary
mkdir -p ~/prj/util/bin
cp target/release/br ~/prj/util/bin/br
chmod +x ~/prj/util/bin/br

echo "Installed: $(br --version 2>/dev/null || echo 'br')"
