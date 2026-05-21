# Shared defaults for mjolnix helper scripts (source, do not execute).

mjolnix_default_data_dir() {
  printf '%s/mjolnix' "${XDG_DATA_HOME:-${HOME}/.local/share}"
}
