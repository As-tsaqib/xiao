#!/system/bin/sh

TERMUX_PREFIX=/data/data/com.termux/files/usr
TERMUX_BIN=$TERMUX_PREFIX/bin
TERMUX_WRAPPER_NAMES='xiao xiao-ctl'

is_xiao_wrapper() {
  [ -f "$1" ] && grep -Fqx '# XIAO_MODULE_WRAPPER=1' "$1" 2>/dev/null
}

fix_termux_metadata() {
  metadata_file=$1
  termux_owner=$(stat -c '%u:%g' "$TERMUX_BIN" 2>/dev/null || true)
  [ -z "$termux_owner" ] || chown "$termux_owner" "$metadata_file" 2>/dev/null || true
  chmod 0700 "$metadata_file" 2>/dev/null || true
  if [ -e "$TERMUX_BIN/sh" ]; then
    chcon --reference="$TERMUX_BIN/sh" "$metadata_file" 2>/dev/null || true
  fi
  restorecon "$metadata_file" 2>/dev/null || true
}

install_termux_wrappers() {
  wrapper_template=$MODDIR/termux/xiao-wrapper
  installed=0
  skipped=0
  [ -d "$TERMUX_BIN" ] || {
    xiao_log "Termux tidak ditemukan di $TERMUX_PREFIX."
    return 1
  }
  [ -f "$wrapper_template" ] || return 1

  for wrapper_name in $TERMUX_WRAPPER_NAMES; do
    wrapper_target=$TERMUX_BIN/$wrapper_name
    wrapper_backup=$TERMUX_BIN/.${wrapper_name}.xiao-module.bak
    wrapper_temp=$TERMUX_BIN/.${wrapper_name}.xiao-module.tmp.$$
    if [ -e "$wrapper_target" ] || [ -L "$wrapper_target" ]; then
      if is_xiao_wrapper "$wrapper_target"; then
        rm -f "$wrapper_target"
      elif [ ! -e "$wrapper_backup" ] && [ ! -L "$wrapper_backup" ]; then
        mv "$wrapper_target" "$wrapper_backup"
      else
        xiao_log "Melewati $wrapper_name karena command dan backup lama sama-sama ada."
        skipped=$((skipped + 1))
        continue
      fi
    fi
    cp "$wrapper_template" "$wrapper_temp" || return 1
    fix_termux_metadata "$wrapper_temp"
    mv "$wrapper_temp" "$wrapper_target" || return 1
    installed=$((installed + 1))
  done
  xiao_log "$installed wrapper Termux dipasang, $skipped dilewati."
}

remove_termux_wrappers() {
  [ -d "$TERMUX_BIN" ] || return 0
  for wrapper_name in $TERMUX_WRAPPER_NAMES; do
    wrapper_target=$TERMUX_BIN/$wrapper_name
    wrapper_backup=$TERMUX_BIN/.${wrapper_name}.xiao-module.bak
    if is_xiao_wrapper "$wrapper_target"; then
      rm -f "$wrapper_target"
      if [ -e "$wrapper_backup" ] || [ -L "$wrapper_backup" ]; then
        mv "$wrapper_backup" "$wrapper_target"
      fi
    fi
  done
}

termux_wrappers_status() {
  [ -d "$TERMUX_BIN" ] || { printf 'termux-not-found\n'; return; }
  ready=0
  total=0
  for wrapper_name in $TERMUX_WRAPPER_NAMES; do
    total=$((total + 1))
    is_xiao_wrapper "$TERMUX_BIN/$wrapper_name" && ready=$((ready + 1))
  done
  [ "$ready" -eq "$total" ] && printf 'ready\n' || printf 'partial-%s-of-%s\n' "$ready" "$total"
}
