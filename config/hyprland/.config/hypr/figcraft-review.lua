-- Called only by the approved host command, never on connection or from a keybinding.
local review = {}
local saved
local is_active = false
local WORKSPACE = 6 -- apps/claude.lua owns the cockpit workspace

local function refuse(message)
    hl.notification.create({ text = "Design review: " .. message, time = 5000 })
    error("Design review: " .. message)
end

local function dispatch(action)
    local result = hl.dispatch(action)
    if not result.ok then
        refuse(result.error .. "; arrangement kept for recovery with design-review stop")
    end
end

local function snapshot(window)
    if not window.workspace or not window.monitor or window.group or window.pinned then
        refuse("both windows need a workspace and monitor, and must be ungrouped and unpinned")
    end
    local workspace = window.workspace.id
    if workspace < 0 then
        local name = window.workspace.name
        workspace = name:sub(1, 8) == "special:" and name or "name:" .. name
    end
    return {
        stable_id = window.stable_id, address = window.address, pid = window.pid,
        workspace = workspace,
        floating = window.floating,
        x = window.at.x, y = window.at.y, width = window.size.x, height = window.size.y,
        fullscreen = window.fullscreen, client = window.fullscreen_client,
    }
end

local function set_geometry(window, x, y, width, height)
    dispatch(hl.dsp.window.resize({ window = window, x = width, y = height, relative = false }))
    dispatch(hl.dsp.window.move({ window = window, x = x, y = y, relative = false }))
end

function review.start()
    if saved then
        if not is_active then
            refuse("an incomplete arrangement needs recovery with design-review stop before starting again")
        end
        return
    end
    local cockpits, designs = {}, {}
    local focused = hl.get_active_window()
    for _, window in ipairs(hl.get_windows()) do
        if window.class == "claude-sessions" then
            table.insert(cockpits, window)
        elseif window.class == "ai.storyteller.figcraft" then
            table.insert(designs, window)
        end
    end
    if #cockpits ~= 1 then
        refuse("open exactly one cockpit window first")
    end
    local design
    for _, window in ipairs(designs) do
        if focused and window.stable_id == focused.stable_id then
            design = window
        end
    end
    if not design then
        if #designs ~= 1 then
            refuse("open FigCraft first; with multiple windows, focus the design to review")
        end
        design = designs[1]
    end
    local cockpit = cockpits[1]
    local arrangement = { snapshot(cockpit), snapshot(design) }
    local monitor = cockpit.monitor
    local width, height = monitor.width, monitor.height
    if monitor.transform % 2 == 1 then
        width, height = height, width
    end
    -- The monitor API reports physical pixels; window coordinates and reserved edges are logical.
    -- https://wiki.hypr.land/Configuring/Basics/Dispatchers/
    local reserved = monitor.reserved
    width = math.floor(width / monitor.scale - reserved.left - reserved.right)
    height = math.floor(height / monitor.scale - reserved.top - reserved.bottom)
    if width < 2 or height < 1 then
        refuse("the cockpit monitor has no usable space")
    end
    local left_width = math.floor(width / 2)
    local x, y = monitor.x + reserved.left, monitor.y + reserved.top
    saved = arrangement
    for index, window in ipairs({ cockpit, design }) do
        dispatch(hl.dsp.window.fullscreen_state({ window = window, internal = 0, client = 0, action = "set" }))
        dispatch(hl.dsp.window.float({ window = window, action = "set" }))
        dispatch(hl.dsp.window.move({ window = window, workspace = WORKSPACE, follow = false }))
        local offset = index == 1 and 0 or left_width
        set_geometry(window, x + offset, y, index == 1 and left_width or width - left_width, height)
    end
    dispatch(hl.dsp.focus({ window = cockpit }))
    is_active = true
end

function review.stop()
    if not saved then
        return
    end
    is_active = false
    local windows = hl.get_windows()
    for _, original in ipairs(saved) do
        for _, window in ipairs(windows) do
            -- An address or PID can be reused after a close. Never restore a replacement window.
            if window.stable_id == original.stable_id and window.address == original.address and window.pid == original.pid then
                dispatch(hl.dsp.window.fullscreen_state({ window = window, internal = 0, client = 0, action = "set" }))
                dispatch(hl.dsp.window.float({ window = window, action = "set" }))
                dispatch(hl.dsp.window.move({ window = window, workspace = original.workspace, follow = false }))
                set_geometry(window, original.x, original.y, original.width, original.height)
                dispatch(hl.dsp.window.float({ window = window, action = original.floating and "set" or "unset" }))
                dispatch(hl.dsp.window.fullscreen_state({ window = window, internal = original.fullscreen, client = original.client, action = "set" }))
                break
            end
        end
    end
    saved = nil
end

return review
