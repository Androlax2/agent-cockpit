-- The Storyteller "craft" apps (FigCraft, WordCraft, GridCraft, PdfCraft…): documents and designs,
-- read and edited at full colour, never through the default 0.95 / 0.9 opacity.
-- render_unfocused: a Claude session drives them through their MCP server while their window sits on
-- another workspace; suspended there, FigCraft refuses every command ("la fenêtre doit être visible").
hl.window_rule({ match = { class = "^(ai\\.storyteller\\.[a-z]+craft)$" }, opacity = "1 1", render_unfocused = true })
