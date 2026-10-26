-- The Storyteller "craft" apps (FigCraft, WordCraft, GridCraft, PdfCraft…): documents and designs,
-- read and edited at full colour, never through the default 0.95 / 0.9 opacity.
-- render_unfocused: a Claude session drives them through their MCP server while their window sits on
-- another workspace; suspended there, FigCraft refuses every command ("la fenêtre doit être visible").
hl.window_rule({ match = { class = "^(ai\\.storyteller\\.[a-z]+craft)$" }, opacity = "1 1", render_unfocused = true })

-- craft-open starts FigCraft from the cockpit, where a new window would land on the cockpit's
-- workspace. `hyprctl eval 'FIGCRAFT_OPEN_ON_WORKSPACE(2)'` sends the next FigCraft window that
-- opens within the request's lifetime to workspace 2, without following it there.
--
-- Each checkout keeps its own FigCraft process (its own port and file roots), so tabs are
-- Hyprland's: when workspace 2 already shows one FigCraft (a window or a tab group of them), the
-- new window joins it as a tab instead of splitting the workspace. Several separate FigCraft tiles
-- there, a locked group or a floating one: it opens beside them, as before.
local REQUEST_LIFETIME_S = 30
local FIGCRAFT_CLASS = "ai.storyteller.figcraft"
local workspace_request

local function tab_host(window, workspace)
    local hosts, host = {}, nil
    for _, other in ipairs(hl.get_windows()) do
        if other.class == FIGCRAFT_CLASS and other.address ~= window.address
            and other.workspace and other.workspace.id == workspace
            and not other.floating then
            local identity = other.group and other.group.current and other.group.current.address or other.address
            if not hosts[identity] then
                hosts[identity] = true
                if host then
                    return nil, "several FigCraft windows already share workspace " .. workspace
                end
                host = other
            end
        end
    end
    if host and host.group and (host.group.locked or host.group.denied) then
        return nil, "the FigCraft tabs on workspace " .. workspace .. " are locked"
    end
    return host
end

local function join_as_tab(window, workspace)
    local host, reason = tab_host(window, workspace)
    if not host then
        if reason then
            hl.notification.create({ text = "FigCraft opened beside the others: " .. reason, time = 5000 })
        end
        return
    end
    if not host.group then
        hl.dispatch(hl.dsp.group.toggle({ window = host }))
    end
    if not host.group then
        hl.notification.create({ text = "FigCraft opened beside the other one: Hyprland made no tab group", time = 5000 })
        return
    end
    host.group:add(window)
end

function FIGCRAFT_OPEN_ON_WORKSPACE(workspace)
    if type(workspace) ~= "number" or workspace < 1 or workspace % 1 ~= 0 then
        error("FIGCRAFT_OPEN_ON_WORKSPACE: not a workspace number: " .. tostring(workspace))
    end
    workspace_request = { workspace = workspace, expires_at = os.time() + REQUEST_LIFETIME_S }
end

hl.on("window.open", function(window)
    if window.class ~= FIGCRAFT_CLASS or not workspace_request then
        return
    end
    local request = workspace_request
    workspace_request = nil
    if os.time() > request.expires_at then
        return
    end
    hl.dispatch(hl.dsp.window.move({ window = window, workspace = tostring(request.workspace), follow = false }))
    join_as_tab(window, request.workspace)
end)
