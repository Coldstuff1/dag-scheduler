#!/usr/bin/env bash
set -euo pipefail

WORKSPACE="${1:-.}"
REBUILD_FLAG=""

if [[ "${2:-}" == "--rebuild" || "${1:-}" == "--rebuild" ]]; then
  REBUILD_FLAG="--remove-existing-container"
  [[ "${1:-}" == "--rebuild" ]] && WORKSPACE="."
fi

# Define Neovim + CLI tools + interactive shells (zsh/bash) to inject
EXTRA_FEATURES='{
  "ghcr.io/devcontainers/features/common-utils:2": {},
  "ghcr.io/duduribeiro/devcontainer-features/neovim:1": {},
}'

# 1. Start/rebuild container with:
#    - Bind mount for ~/.config/nvim
#    - Named Docker volume 'nvim-devcontainer-data' for plugins/Mason/state
devcontainer up \
  --workspace-folder "$WORKSPACE" \
  $REBUILD_FLAG \
  --additional-features "$EXTRA_FEATURES" \
  --mount "type=bind,source=${HOME}/.config/nvim,target=/tmp/host-nvim-config" \
  --mount "type=volume,source=nvim-devcontainer-data,target=/tmp/nvim-data"

# 2. Wire up config and persistent plugin/state directories for the active container user
devcontainer exec --workspace-folder "$WORKSPACE" sh -c '
  mkdir -p "$HOME/.config" "$HOME/.local/share" "$HOME/.local/state"

  # Link Neovim config
  if [ ! -e "$HOME/.config/nvim" ]; then
    ln -s /tmp/host-nvim-config "$HOME/.config/nvim"
  fi

  # Ensure container user owns the persistent Docker volume
  if command -v sudo >/dev/null 2>&1; then
    sudo chown -R "$(id -u):$(id -g)" /tmp/nvim-data 2>/dev/null || true
  fi
  mkdir -p /tmp/nvim-data/share /tmp/nvim-data/state

  # Link Lazy/Mason/Treesitter data (~/.local/share/nvim)
  if [ ! -L "$HOME/.local/share/nvim" ]; then
    rm -rf "$HOME/.local/share/nvim"
    ln -s /tmp/nvim-data/share "$HOME/.local/share/nvim"
  fi

  # Link Neovim state/shada/logs (~/.local/state/nvim)
  if [ ! -L "$HOME/.local/state/nvim" ]; then
    rm -rf "$HOME/.local/state/nvim"
    ln -s /tmp/nvim-data/state "$HOME/.local/state/nvim"
  fi
'

# 3. Launch nvim with zsh/bash set as SHELL so :terminal has arrow-key history
exec devcontainer exec --workspace-folder "$WORKSPACE" sh -lc '
  export PATH="$PATH:/usr/local/bin:/opt/nvim-linux64/bin:/opt/nvim-linux-x86_64/bin:/opt/nvim-linux-arm64/bin"
  if command -v zsh >/dev/null 2>&1; then
    export SHELL="$(command -v zsh)"
  elif command -v bash >/dev/null 2>&1; then
    export SHELL="$(command -v bash)"
  fi
  exec nvim "$@"
' -- "${@:2}"


