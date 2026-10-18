#!/usr/bin/env bash
# make handoff-install: install what Claude left in .handoff/ for the places greywall keeps it
# from writing (config/claude, config/fish, config/systemd). Each group is a folder with the files,
# their SHA256SUMS and a MANIFEST, one tab-separated line per file:
#   <file>	<destination, relative to the repository>	[<command to run after installing>]
# Per group: every file must be in SHA256SUMS and match it, or nothing of the group is installed;
# a destination outside the repository is refused; the diff against the destination is shown and
# the group is installed only on "y"; each follow-up command runs once; the group is then removed.
# This runs with your full rights, outside the sandbox: read the diffs before answering.
# HANDOFF_REPO points at another repository (tests).
set -euo pipefail
repo=${HANDOFF_REPO:-$(cd "$(dirname "$0")/.." && pwd)}
handoff="$repo/.handoff"
status=0

[[ -d $handoff ]] || { echo "nothing to install: no $handoff"; exit 0; }
shopt -s nullglob
for group_dir in "$handoff"/*/; do
    group=$(basename "$group_dir")
    manifest="$group_dir/MANIFEST"
    if [[ ! -f $manifest ]]; then
        echo "== $group: no MANIFEST, skipped (install it by hand)"
        continue
    fi
    echo "== $group"
    files=() destinations=() afters=() problem=""
    while IFS=$'\t' read -r file destination after || [[ -n $file ]]; do
        [[ -z $file || $file == \#* ]] && continue
        if [[ $file == */* || $destination == /* || $destination == *..* || -z $destination ]]; then
            problem="refusing $file -> $destination (a plain file name, and a destination inside the repository)"
            break
        fi
        # The destination, symlinks resolved, must stay inside the repository: a link planted in it
        # (config/x -> ~/.ssh) would make the copy land elsewhere while the diff shows the repo path.
        resolved=$(realpath -m -- "$repo/$destination")
        if [[ $resolved != "$repo/"* || -L "$repo/$destination" ]]; then
            problem="refusing $destination: it resolves outside the repository or is a symlink"
            break
        fi
        if ! awk -v name="$file" '$2 == name { found = 1 } END { exit !found }' "$group_dir/SHA256SUMS" 2>/dev/null; then
            problem="$file is not in SHA256SUMS"
            break
        fi
        files+=("$file"); destinations+=("$destination"); afters+=("${after:-}")
    done < "$manifest"
    if [[ -z $problem ]] && ! (cd "$group_dir" && sha256sum --check --quiet SHA256SUMS); then
        problem="checksum mismatch"
    fi
    if [[ -n $problem ]]; then
        echo "   not installed: $problem"
        status=1
        continue
    fi
    changed=0
    for index in "${!files[@]}"; do
        source="$group_dir/${files[$index]}" target="$repo/${destinations[$index]}"
        if ! cmp -s "$source" "$target"; then
            changed=1
            # Shown with control characters made visible (cat -v): nothing hides in the review.
            # A new file is shown whole, against /dev/null.
            current=$target
            [[ -e $target ]] || { current=/dev/null; echo "   new file: ${destinations[$index]}"; }
            diff -u --label "${destinations[$index]} (now)" --label "${destinations[$index]} (handoff)" "$current" "$source" | cat -v || true
        fi
    done
    if (( ! changed )); then
        echo "   already installed; removing the handoff"
        rm -rf "$group_dir"
        continue
    fi
    declare -A shown=()
    for after in "${afters[@]}"; do
        [[ -z $after || -n ${shown[$after]:-} ]] && continue
        shown[$after]=1
        echo "   then runs: $(cat -v <<< "$after")"
    done
    unset shown
    read -r -p "Install $group? [y/N] " answer || answer=""
    if [[ $answer != y && $answer != Y ]]; then
        echo "   left as is"
        continue
    fi
    for index in "${!files[@]}"; do
        mkdir -p "$(dirname "$repo/${destinations[$index]}")"
        cp "$group_dir/${files[$index]}" "$repo/${destinations[$index]}"
        echo "   installed ${destinations[$index]}"
    done
    declare -A ran=()
    for after in "${afters[@]}"; do
        [[ -z $after || -n ${ran[$after]:-} ]] && continue
        ran[$after]=1
        echo "   running: $after"
        (cd "$repo" && bash -c "$after") || { echo "   failed: $after"; status=1; }
    done
    unset ran
    rm -rf "$group_dir"
done
exit "$status"
