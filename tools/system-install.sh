#!/usr/bin/env bash
# make system: copy system/ into /, as root, one reviewed file at a time. Claude sessions in the
# dotfiles can write system/, and what lands in /etc runs as root: every changed file shows its
# diff (control characters made visible, so nothing hides in it) and is installed only on "y";
# a file asking for setuid, setgid or sticky bits is refused outright.
# SYSTEM_ROOT and SYSTEM_SOURCE point elsewhere, and SUDO="" runs without sudo (tests).
set -uo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
source_dir=${SYSTEM_SOURCE:-$repo/system}
root=${SYSTEM_ROOT:-}
sudo=${SUDO-sudo}
installed=0

while IFS= read -r -d '' -u 3 file; do
    path=${file#"$source_dir"/}
    target="$root/$path"
    cmp -s "$file" "$target" && continue
    mode=$(stat -c %a "$file")
    if (( 8#$mode & 8#7000 )); then
        echo "== /$path: refused, it asks for mode $mode (setuid, setgid or sticky)"
        continue
    fi
    if [[ -L $file ]]; then
        echo "== /$path: refused, a symlink in system/"
        continue
    fi
    echo "== /$path (mode $mode)"
    # A new file is shown whole (against /dev/null): a planted sudoers entry is a new file.
    current=$target
    [[ -e $target ]] || { current=/dev/null; echo "   new file:"; }
    diff -u --label "/$path (now)" --label "system/$path" "$current" "$file" | cat -v
    read -r -p "Install /$path? [y/N] " answer || answer=""
    if [[ $answer != y && $answer != Y ]]; then
        echo "   left as is"
        continue
    fi
    $sudo install -D -o root -g root -m "$mode" "$file" "$target" && installed=$((installed + 1)) && echo "   installed"
done 3< <(find "$source_dir" \( -type f -o -type l \) -print0 | sort -z)

(( installed > 0 )) && [[ -z $root ]] && $sudo systemctl daemon-reload
exit 0
