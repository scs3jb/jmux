#!/usr/bin/env zsh
# jmux ZDOTDIR bootstrap — auto-injects shell integration, then restores
# the user's original ZDOTDIR so their own .zshenv/.zshrc run normally.
#
# How it works:
#   jmux sets ZDOTDIR to this directory. Zsh loads this .zshenv first.
#   We source the integration script, restore the real ZDOTDIR, then
#   source the user's actual .zshenv if it exists.

# Save our directory and restore the user's ZDOTDIR
_jmux_integration_dir="${ZDOTDIR}"

if [[ -n "$JMUX_ZSH_ZDOTDIR" ]]; then
  ZDOTDIR="$JMUX_ZSH_ZDOTDIR"
  unset JMUX_ZSH_ZDOTDIR
elif [[ -n "$JMUX_ZSH_ORIGINAL_ZDOTDIR" ]]; then
  # Sentinel value meaning "ZDOTDIR was unset"
  if [[ "$JMUX_ZSH_ORIGINAL_ZDOTDIR" == "__jmux_unset__" ]]; then
    unset ZDOTDIR
  else
    ZDOTDIR="$JMUX_ZSH_ORIGINAL_ZDOTDIR"
  fi
  unset JMUX_ZSH_ORIGINAL_ZDOTDIR
else
  unset ZDOTDIR
fi

# Prepend jmux bin directory to PATH if it exists
_jmux_bin_dir="${_jmux_integration_dir}/../bin"
if [[ -d "$_jmux_bin_dir" ]]; then
  _jmux_bin_dir="${_jmux_bin_dir:A}"  # Resolve to absolute path
  [[ ":$PATH:" != *":${_jmux_bin_dir}:"* ]] && export PATH="${_jmux_bin_dir}:$PATH"
fi
unset _jmux_bin_dir

# Source the jmux integration
if [[ -f "${_jmux_integration_dir}/jmux-zsh-integration.zsh" ]]; then
  source "${_jmux_integration_dir}/jmux-zsh-integration.zsh"
fi
unset _jmux_integration_dir

# Now source the user's real .zshenv if it exists
if [[ -n "$ZDOTDIR" ]]; then
  [[ -f "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"
else
  [[ -f "$HOME/.zshenv" ]] && source "$HOME/.zshenv"
fi
