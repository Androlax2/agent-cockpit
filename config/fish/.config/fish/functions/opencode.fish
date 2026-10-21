# Like claude: a bare interactive `opencode` asks where to run, the session manager (Enter or m,
# its card like a Claude session's), the existing cockpit in this terminal (r), or attaches
# the same managed process here (t). With arguments (run, serve, auth, a project) and in the
# manager's own launches it runs right here, through greywall-agents/opencode.
function opencode
    if test -z "$COCKPIT_INSIDE"; and isatty stdin; and isatty stdout; and test (count $argv) -eq 0
        set -l prompt (set_color --bold blue)'opencode'(set_color normal)'  '(set_color --bold)'⏎'(set_color normal)' session manager  '(set_color --bold)'r'(set_color normal)' resume session  '(set_color --bold)'t'(set_color normal)' this terminal  '
        read -l -n 1 -P $prompt choice; or return 130
        switch $choice
            case '' m M
                cockpit --agent opencode new (pwd); and cockpit show
                return
            case r R
                cockpit attach
                return
            case t T
                cockpit --agent opencode terminal (pwd)
                return
            case '*'
                echo "opencode: cancelled"
                return 1
        end
    end
    command opencode $argv
end
