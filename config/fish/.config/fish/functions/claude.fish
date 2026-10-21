function claude
    set -lx CLAUDE_AUTO_RETRY_ACTIVE 1
    # An interactive `claude` from any terminal asks where to run: Enter (or m) opens it in the
    # session manager on its workspace (~/.local/bin/cockpit), t runs it in this terminal,
    # any other key (or Ctrl-C) cancels. The manager's own viewer pane (COCKPIT_INSIDE),
    # print mode and the informational flags run claude right here without asking.
    if test -z "$COCKPIT_INSIDE"; and isatty stdin; and isatty stdout
        and not contains -- -p $argv; and not contains -- --print $argv
        and not contains -- --help $argv; and not contains -- --version $argv
        set -l prompt (set_color --bold blue)'claude'(set_color normal)'  '(set_color --bold)'⏎'(set_color normal)' session manager  '(set_color --bold)'t'(set_color normal)' this terminal  '
        read -l -n 1 -P $prompt choice; or return 130
        switch $choice
            case '' m M
                cockpit --agent claude new (pwd) $argv; and cockpit show
                return
            case t T
                # Run right here, below. The marker reaches the session (the retry wrapper carries the
                # environment): its card says "terminal", and the cockpit's restart-all, restore,
                # queue, error hand-off and ask-to-guide leave it alone. ask-to-run still asks for approval.
                set -fx COCKPIT_TERMINAL 1
            case '*'
                echo "claude: cancelled"
                return 1
        end
    end
    node (npm root -g)/claude-auto-retry/src/launcher.js $argv
end
