-- The Storyteller "craft" apps (FigCraft, WordCraft, GridCraft, PdfCraft…): documents and designs,
-- read and edited at full colour, never through the default 0.95 / 0.9 opacity.
-- render_unfocused: a Claude session drives them through their MCP server while their window sits on
-- another workspace; suspended there, FigCraft refuses every command ("la fenêtre doit être visible").
hl.window_rule({ match = { class = "^(ai\\.storyteller\\.[a-z]+craft)$" }, opacity = "1 1", render_unfocused = true })

-- craft-open starts FigCraft from the cockpit, where a new window would land on the cockpit's
-- workspace. `hyprctl eval 'FIGCRAFT_OPEN_ON_WORKSPACE(2)'` sends the next FigCraft window that
-- opens within the request's lifetime to workspace 2, without following it there.
local REQUEST_LIFETIME_S = 30
local workspace_request

function FIGCRAFT_OPEN_ON_WORKSPACE(workspace)
    if type(workspace) ~= "number" or workspace < 1 or workspace % 1 ~= 0 then
        error("FIGCRAFT_OPEN_ON_WORKSPACE: not a workspace number: " .. tostring(workspace))
    end
    workspace_request = { workspace = workspace, expires_at = os.time() + REQUEST_LIFETIME_S }
end

hl.on("window.open", function(window)
    if window.class ~= "ai.storyteller.figcraft" or not workspace_request then
        return
    end
    local request = workspace_request
    workspace_request = nil
    if os.time() > request.expires_at then
        return
    end
    hl.dispatch(hl.dsp.window.move({ window = window, workspace = tostring(request.workspace), follow = false }))
end)
