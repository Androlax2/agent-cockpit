# Shared host-side discovery; invoked with Bash by the POSIX launchers, sourced by craft-open.
# Reservations are never recycled: a sandbox retains its forward after the app exits.
figcraft_home="$HOME/.config/greywall-agents/figcraft"

figcraft_arguments() {
    process_arguments=()
    [[ -r /proc/$1/cmdline ]] || return 1
    mapfile -d '' -t process_arguments < "/proc/$1/cmdline" 2>/dev/null
}

figcraft_confined_port() { # pid root
    local index argument control="" read_count=0 write_count=0 control_count=0 environment_entry has_private_config=false
    figcraft_arguments "$1" || return 1
    for (( index=0; index < ${#process_arguments[@]}; index++ )); do
        argument=${process_arguments[index]}
        case "$argument" in
            --control)
                control=${process_arguments[index+1]:-}
                (( control_count+=1 )) ;;
            --automation-read-root)
                [[ ${process_arguments[index+1]:-} == "$2" ]] || return 1
                (( read_count+=1 )) ;;
            --automation-write-root)
                [[ ${process_arguments[index+1]:-} == "$2" ]] || return 1
                (( write_count+=1 )) ;;
            --automation-*=*|--control=*) return 1 ;;
        esac
    done
    [[ $control_count == 1 && $read_count == 1 && $write_count == 1 ]] || return 1
    [[ $control =~ ^[1-9][0-9]{0,4}$ ]] && (( control <= 65535 )) || return 1
    # Matching file roots alone are insufficient: startup recovers documents before applying them.
    [[ -r /proc/$1/environ ]] || return 1
    while IFS= read -r -d '' environment_entry; do
        [[ $environment_entry == "XDG_CONFIG_HOME=$figcraft_home/instances/$control/config" ]] && has_private_config=true
    done < "/proc/$1/environ"
    [[ $has_private_config == true ]] || return 1
    printf '%s\n' "$control"
}

figcraft_reserved_port() { # root
    local reservation
    for reservation in "$figcraft_home"/ports/*; do
        [[ -f $reservation && $(cat "$reservation") == "$1" ]] || continue
        printf '%s\n' "${reservation##*/}"
        return 0
    done
    return 1
}

figcraft_port_for_root() { # root
    local reserved_port process_id control_port
    reserved_port=$(figcraft_reserved_port "$1") || return 0
    for process_id in $(pgrep -x figcraft || true); do
        control_port=$(figcraft_confined_port "$process_id" "$1") || continue
        [[ $control_port == "$reserved_port" ]] || continue
        printf '%s\n' "$control_port"
        return 0
    done
}

figcraft_foreign_holders() { # port root
    local process_id index directory repository
    for process_id in $(pgrep -x greywall || true); do
        figcraft_arguments "$process_id" || continue
        for (( index=0; index < ${#process_arguments[@]}; index++ )); do
            case "${process_arguments[index]}" in
                --) break ;;
                --forward|-f) [[ ${process_arguments[index+1]:-} == "$1" ]] || continue ;;
                --forward="$1"|-f="$1"|-f"$1") ;;
                *) continue ;;
            esac
            directory=$(readlink -e "/proc/$process_id/cwd") || {
                echo "unknown folder (sandbox $process_id)"
                break
            }
            repository=$(git -C "$directory" rev-parse --show-toplevel 2>/dev/null || printf '%s' "$directory")
            [[ $repository == "$2" ]] || printf '%s\n' "$directory"
            break
        done
    done
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    set -euo pipefail
    figcraft_port_for_root "$1"
fi
