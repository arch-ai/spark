#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${1:-https://github.com/arch-ai/spark}"
CLONE_DIR="${CLONE_DIR:-}"

if ! command -v git >/dev/null 2>&1; then
  echo "git not found. Please install git first."
  exit 1
fi

if [ -z "$CLONE_DIR" ]; then
  CLONE_DIR="$(mktemp -d)"
  trap 'rm -rf "$CLONE_DIR"' EXIT
fi

TARGET_DIR="$CLONE_DIR/spark"

echo "Cloning $REPO_URL..."
git clone --depth 1 "$REPO_URL" "$TARGET_DIR"

echo "Running installer..."
cd "$TARGET_DIR"
bash install.sh
