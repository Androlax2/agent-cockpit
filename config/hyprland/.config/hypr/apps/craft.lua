-- The Storyteller "craft" apps (FigCraft, WordCraft, GridCraft, PdfCraft…): documents and designs,
-- read and edited at full colour, never through the default 0.95 / 0.9 opacity.
hl.window_rule({ match = { class = "^(ai\\.storyteller\\.[a-z]+craft)$" }, opacity = "1 1" })
