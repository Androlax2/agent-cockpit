# Renders `make help` from annotations in the Makefile:
#   ##@ Section title
#   target: prerequisites ## [tag] Description
# A documented target that another documented target depends on is drawn
# indented, so `setup`, `check`, `dump` and `clean` show the steps they run.
# Run with -v use_color=1 to colour section titles, target names and tags.

BEGIN {
    if (use_color) {
        bold = "\033[1m"
        dim = "\033[2m"
        reset = "\033[0m"
        cyan = "\033[36m"
        tag_color["read-only"] = "\033[32m"
        tag_color["sudo"] = "\033[33m"
        tag_color["asks"] = "\033[33m"
        tag_color["no undo"] = "\033[31m"
    }
}

/^##@ / {
    entry_count++
    entry_is_section[entry_count] = 1
    entry_name[entry_count] = substr($0, 5)
    next
}

/^[a-zA-Z0-9_-]+:[^=].*## / {
    marker = index($0, "## ")
    rule = substr($0, 1, marker - 1)
    description = substr($0, marker + 3)
    colon = index(rule, ":")
    name = substr(rule, 1, colon - 1)

    prerequisite_count = split(substr(rule, colon + 1), prerequisites, " ")
    for (i = 1; i <= prerequisite_count; i++) {
        is_step[prerequisites[i]] = 1
    }

    tag = ""
    if (match(description, /^\[[^]]+\] /)) {
        tag = substr(description, 2, RLENGTH - 3)
        description = substr(description, RLENGTH + 1)
    }

    entry_count++
    entry_name[entry_count] = name
    entry_description[entry_count] = description
    entry_tag[entry_count] = tag
    if (length(name) > name_width) {
        name_width = length(name)
    }
    if (tag != "" && length(description) > tagged_description_width) {
        tagged_description_width = length(description)
    }
}

function print_heading(text) {
    printf "\n%s%s%s\n", bold, text, reset
}

function pad(text, width) {
    return sprintf("%-" width "s", text)
}

function print_row(indent, name, description) {
    printf "%s%s%s%s%s", indent, cyan, pad(name, name_width + 6 - length(indent)), reset, description
}

function print_tag(tag) {
    printf "%s[%s]%s", tag_color[tag], tag, reset
}

END {
    printf "%sdotfiles%s  rebuild, audit and clean this Arch Linux machine\n", bold, reset

    print_heading("Usage")
    print_row("  ", "make <target>", "run a target, for example make check")
    printf "\n"
    print_row("  ", "make", "show this page")
    printf "\n"

    for (i = 1; i <= entry_count; i++) {
        if (entry_is_section[i]) {
            print_heading(entry_name[i])
            continue
        }
        indent = is_step[entry_name[i]] ? "    " : "  "
        if (entry_tag[i] == "") {
            print_row(indent, entry_name[i], entry_description[i])
        } else {
            print_row(indent, entry_name[i], pad(entry_description[i], tagged_description_width + 2))
            print_tag(entry_tag[i])
        }
        printf "\n"
    }

    print_heading("Tags")
    printf "  "; print_tag("sudo");      printf "       asks for your password\n"
    printf "  "; print_tag("asks");      printf "       confirms before deleting anything\n"
    printf "  "; print_tag("read-only"); printf "  changes nothing\n"
    printf "  "; print_tag("no undo");   printf "    deletes without asking\n"

    print_heading("Common workflows")
    print_row("  ", "New machine", "make setup, then the manual steps in README.md")
    printf "\n"
    print_row("  ", "Track changes", "make check, make dump, then review git diff and commit")
    printf "\n"
    print_row("  ", "Free up space", "make disk, then make clean")
    printf "\n"
}
