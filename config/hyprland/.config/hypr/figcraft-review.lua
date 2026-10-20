-- Invoked only by the approved host command; neither main window is rearranged.
local review = {}
local PREFIX = "figcraft-chat."
local pending = {}

local function refuse(message)
    hl.notification.create({ text = "Design review: " .. message, time = 5000 })
    error("Design review: " .. message)
end

local function valid_id(id)
    return type(id) == "string" and id:match("^[%w_-]+$") ~= nil
end

local function dispatch(action)
    local result = hl.dispatch(action)
    if not result.ok then
        refuse(result.error)
    end
end

local function workspace_selector(workspace)
    if workspace.id >= 0 then
        return tostring(workspace.id)
    end
    return workspace.name:sub(1, 8) == "special:" and workspace.name or "name:" .. workspace.name
end

local function select_design(address)
    local designs, selected = {}, nil
    local focused = hl.get_active_window()
    for _, window in ipairs(hl.get_windows()) do
        if window.class == "ai.storyteller.figcraft" then
            table.insert(designs, window)
            if address and window.address == address or not address and focused and window.stable_id == focused.stable_id then
                selected = window
            end
        end
    end
    if not address and not selected and #designs == 1 then
        selected = designs[1]
    end
    if not selected then
        refuse("open FigCraft first; with multiple designs, pass --figcraft-window with the intended window address")
    end
    if not selected.workspace or not selected.monitor then
        refuse("the selected FigCraft window has no workspace or monitor")
    end
    return selected
end

local function geometry(design)
    local monitor = design.monitor
    local width, height = monitor.width, monitor.height
    if monitor.transform % 2 == 1 then
        width, height = height, width
    end
    local reserved = monitor.reserved
    width = math.floor(width / monitor.scale - reserved.left - reserved.right)
    height = math.floor(height / monitor.scale - reserved.top - reserved.bottom)
    if width < 1 or height < 2 then
        refuse("the FigCraft monitor has no usable space")
    end
    local chat_width = math.min(560, width)
    return {
        width = chat_width, height = math.floor(height * 0.85),
        x = reserved.left + width - chat_width - math.min(24, width - chat_width),
        y = reserved.top + math.floor(height * 0.08),
        monitor_x = monitor.x, monitor_y = monitor.y,
    }
end

hl.on("window.open", function(window)
    if window.class:sub(1, #PREFIX) ~= PREFIX or not valid_id(window.class:sub(#PREFIX + 1)) then
        return
    end
    local request = pending[window.class]
    if request and request.cancelled then
        dispatch(hl.dsp.window.close({ window = window }))
        return
    end
    local design
    for _, candidate in ipairs(hl.get_windows()) do
        if candidate.class == window.class and candidate.stable_id ~= window.stable_id then
            dispatch(hl.dsp.window.close({ window = window }))
            return
        end
        if request and candidate.stable_id == request.design_id and candidate.class == "ai.storyteller.figcraft" then
            design = candidate
        end
    end
    if not request then
        return
    end
    pending[window.class] = nil
    if not design or not design.workspace or not design.monitor then
        dispatch(hl.dsp.window.close({ window = window }))
        hl.notification.create({ text = "Design review: FigCraft closed before the chat opened; retry when ready", time = 5000 })
        return
    end
    local bounds = geometry(design)
    dispatch(hl.dsp.window.move({ window = window, workspace = workspace_selector(design.workspace), follow = false }))
    dispatch(hl.dsp.window.resize({ window = window, x = bounds.width, y = bounds.height, relative = false }))
    dispatch(hl.dsp.window.move({ window = window, x = bounds.monitor_x + bounds.x, y = bounds.monitor_y + bounds.y, relative = false }))
    dispatch(hl.dsp.focus({ window = window }))
end)

function review.start(id, target, address)
    if not valid_id(id) or type(target) ~= "string" or not target:match("^claude%-retry%-[%w_-]+$")
        or address ~= nil and (type(address) ~= "string" or not address:match("^0x%x+$")) then
        refuse("invalid conversation or agent tmux session")
    end
    local design = select_design(address)
    local class = PREFIX .. id
    for _, window in ipairs(hl.get_windows()) do
        if window.class == class then
            if workspace_selector(window.workspace) ~= workspace_selector(design.workspace) then
                dispatch(hl.dsp.window.move({ window = window, workspace = workspace_selector(design.workspace), follow = false }))
            end
            dispatch(hl.dsp.focus({ window = window }))
            return
        end
    end
    local request = pending[class]
    -- A failed asynchronous Kitty startup must not prevent a later explicit retry.
    if request and not request.cancelled and os.time() - request.requested_at < 10 then
        return
    end
    local bounds = geometry(design)
    pending[class] = { design_id = design.stable_id, requested_at = os.time() }
    -- Both interpolated arguments are restricted to shell-safe identifiers above.
    local ok, reason = pcall(hl.exec_cmd,
        "kitty --class " .. class .. " -e env -u TMUX tmux attach-session -t =" .. target, {
            workspace = workspace_selector(design.workspace) .. " silent",
            float = true,
            size = bounds.width .. " " .. bounds.height,
            move = bounds.x .. " " .. bounds.y,
        })
    if not ok then
        pending[class] = nil
        refuse("could not launch the chat: " .. tostring(reason))
    end
end

function review.stop(id)
    if id ~= nil and not valid_id(id) then
        refuse("invalid conversation ID")
    end
    local classes = {}
    if id then
        classes[PREFIX .. id] = true
    else
        for _, window in ipairs(hl.get_windows()) do
            if window.class:sub(1, #PREFIX) == PREFIX and valid_id(window.class:sub(#PREFIX + 1)) then
                classes[window.class] = true
            end
        end
        for class, request in pairs(pending) do
            if not request.cancelled then
                classes[class] = true
            end
        end
        local count = 0
        for _ in pairs(classes) do count = count + 1 end
        if count > 1 then
            refuse("multiple conversation chats are open; pass the session ID to stop")
        end
    end
    for class in pairs(classes) do
        -- Other slow clients may still map after the first one cleared its launch request.
        pending[class] = { cancelled = true }
        for _, window in ipairs(hl.get_windows()) do
            if window.class == class then
                dispatch(hl.dsp.window.close({ window = window }))
            end
        end
    end
end

return review
