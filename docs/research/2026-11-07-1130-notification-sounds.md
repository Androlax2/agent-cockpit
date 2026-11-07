# Notification sounds: what comparable tools use, and what Torlek could ship

Research for issue [#1130](https://github.com/Androlax2/agent-cockpit/issues/1130)
(Alpha 7, owner request from Theo, 2026-11-07: "I'd like to create custom sounds for each
notification, I don't really like the default one."). It follows the layout of the other
reports in this folder. **Research and proposal.** Merging this document accepts research
delivery only: it adds no audio file, no product code and no dependency, it does not implement
#1130 (which waits on #1127) and it leaves #1130 open. Every recommendation below is a
proposal for Theo to accept or change.

## The question

Today each notification event plays a fixed freedesktop sound name from the owner's sound
theme (#1130 lists `dialog-error`, `dialog-warning`, `message-new-instant`,
`alarm-clock-elapsed`, `complete`). The events are failure, hold, question, quota and finished.
Theo dislikes the default theme sounds and wants his own per event. Three things to learn:

1. What do Herdr and comparable tools play, where do those sounds come from, and under what
   licence?
2. Could Torlek ship or recommend them?
3. What makes a notification sound good, so Theo's own set (and any default set) works?

Not checked: `crates/cockpit-notify` is not in this checkout (HEAD `728c3fff`), so the event
names and default sound names are taken from the text of #1130, not from code.

## What the surveyed tools do

Read on 2026-11-07 unless a date says otherwise. "Origin" is where the sound file came from.

| Tool | Events with a sound | Sounds | Origin and licence of the audio | Owner can change it |
| --- | --- | --- | --- | --- |
| Herdr | done, request (2) | `assets/sounds/done.mp3` (34,560 bytes), `request.mp3` (46,811 bytes), embedded in the binary | Not stated. The repository is Apache-2.0; no separate licence for the audio was found. Added 2026-03-27 | Yes: `ui.sound.path` (all), `ui.sound.done_path`, `ui.sound.request_path`, mp3 only; `ui.sound.agents.<name>` is `default`, `on` or `off` per agent; plays only for background workspaces. A file that is not mp3 is reported and the default sound plays |
| OpenCode | default, question, permission, error, subagent done (5) | 45 sounds, each as mp3 and aac, in `packages/ui/src/assets/audio`, in five families: `alert` (10), `bip-bop` (10), `nope` (12), `staplebops` (7), `yup` (6). Defaults: `bip-bop-01` general, `bip-bop-03` question, `staplebops-06` permission, `nope-03` error, `yup-01` subagent done | Not stated in what was read. The repository is MIT. Notifications and sounds ship disabled in v1 (PR merged 2026-05-13) | Sound packs can be registered by plugins; the TUI config has an `attention` block. Community plugins add more (below) |
| Zed | agent done and input required, one shared sound | `assets/sounds/agent_done.wav` (459,980 bytes), added 2025-05-27 | Not stated. Zed's repository mixes licences (GitHub reports none for the whole; a `LICENSE-GPL` file exists) | On or off only: `play_sound_when_agent_done` is `never` (default), `when_hidden` or `always`. No custom file |
| Claude Code | None of its own | Desktop notification or terminal bell, depending on terminal | Not applicable | `preferredNotifChannel` (`terminal_bell` and others); a `Notification` hook runs any command, and the official example is `afplay /System/Library/Sounds/Glass.aiff` |
| Codex | None of its own | `tui.notification_method` is `auto`, `osc9` or `bel`; `tui.notification_condition` is `unfocused` or `always` | Not applicable | The `notify` program receives `agent-turn-complete` as JSON and can play anything; project config ignores `notify` |
| Warp | None found | Toasts and desktop alerts for completion, errors and waiting for input | Not applicable | No sound setting found in Warp's documentation |
| Ghostty | Terminal bell | `bell-features` includes `audio`, `system`, `attention`, `title`, `border` | The sound is the owner's file or the system alert | `bell-audio-path` (absolute, `~/` or config-relative) and `bell-audio-volume` (0.0 to 1.0, default 0.5); since 1.2.0 on GTK and 1.3.0 on macOS |
| kitty | Terminal bell | System bell through libcanberra, or the compositor's bell on Wayland | System sound theme | `bell_path` (WAV or OGA on Linux, AIFF, MP3 or WAV on macOS), `linux_bell_theme`, `enable_audio_bell`, `command_on_bell` |
| iTerm2 | Terminal bell | System bell sound | macOS | "Silence bell" switch; a trigger action "Ring Bell" plays the standard system bell and "Post Notification" posts to Notification Center. No custom bell sound is documented on the pages read |

Findings that matter for #1130:

- **No surveyed tool lets the owner pick a file for five distinct events.** Herdr has two file
  keys, OpenCode five built-in ids, Zed one shared sound. Torlek's five-event, per-file
  setting is finer than all of them.
- **Tools that ship sounds ship very few, and say nothing about their licence.** Herdr, OpenCode
  and Zed embed their own audio without a visible origin or licence for the files. Torlek
  cannot treat "a popular tool ships it" as a licence.
- **OpenCode's families are a usable vocabulary.** Its names (`yup` for done, `nope` for
  error, `bip-bop` for neutral and question, `staplebops` for permission) show the same
  event-to-character mapping Torlek needs, with several variants per event.
- **Herdr falls back, #1130 refuses.** Herdr reports a non-mp3 path and plays the default.
  #1130 asks for a refusal at configuration load, which is the stricter rule in the user's
  global guidance ("fail loud"). Keep #1130 as written.
- **A second sound source can double the beep.** A Herdr discussion notes that the terminal
  bell is a separate path from Herdr's completion sounds, so muting agent sounds still leaves
  bells. Claude Code's `terminal_bell` and Codex's `bel` method are such paths. Torlek's
  notifier should not also trigger a bell in the session's terminal; this is a note for the
  #1127 and #1130 authors, not a finding about the current code.
- **Level matching is real work.** One user's dotfiles moved Claude Code, Devin and Codex hook
  sounds into Herdr's settings, then changed the amplitude of both files five times (30%, 10%,
  20%, 26.7% of the original). Any set Torlek ships or recommends should come already
  level-matched.

### Community setups around Claude Code and OpenCode

| Setup | What it uses | Licence and origin |
| --- | --- | --- |
| Blog post by alexop.dev, 2026-02-11 | Age of Empires clips on four hooks (`SessionStart` horn, `UserPromptSubmit` "yes", `Stop` "All hail", `PreCompact` "wololo"), played with `afplay ... &` | Game audio of unstated provenance; the post's CC BY 4.0 footer covers the text, not the clips. Not shippable |
| `StaticB1/claude-code-sounds` | One `notification.wav` on the `PermissionRequest` hook | MIT repository; audio origin not stated |
| `opencode-plugin-boops` v2.7.0 | 448 named sounds (`pristine`, `relax`, ...), per-event TOML, local path or URL allowed | Plugin MIT; sounds from notificationsounds.com under CC BY (attribution) |
| `opencode-starcraft` | StarCraft voice lines for `session.idle` and `permission.asked` | Downloads game audio on first run and distributes none: the author avoids shipping it |
| `opencode-alert`, `opencode-notify` (retired) | macOS system sounds (`Glass`, `Submarine`, `Blow`) or small `.wav` files | System sounds, not redistributable |

The common pattern is a hook that runs a player in the background, with the sound as the
owner's choice and the tool shipping nothing, or shipping an unexplained handful. Game audio
is popular among individuals and is the one thing the authors themselves avoid distributing.

Chat tools such as Slack offer a fixed list of built-in sounds (the default is "Knock Brush");
whether custom uploads are supported is disputed in the secondary sources found, so they are
not used as evidence. CI tools were not surveyed: their notifications go through chat or email.

## Where sounds can come from, and whether Torlek could use them

The repository is Apache-2.0. Three licence questions decide everything: may the file be
redistributed in a repository or release, is attribution needed, and does the licence force
terms on the project.

| Source | Licence (as found) | Could Torlek ship the files? | Could Torlek recommend them? |
| --- | --- | --- | --- |
| freedesktop sound theme (`sound-theme-freedesktop` 0.8-6) | A mix: CC-BY-3.0, CC-BY-4.0, CC-BY-SA-3.0, GPL-2.0-only, GPL-2.0-or-later, LGPL-2.0-or-later (Arch Linux package metadata, built 2024-07-05) | No. The share-alike and GPL parts do not fit an Apache-2.0 tree. Referring to the sounds by name, as today, ships nothing | Yes, as the fallback |
| Kenney "Interface Sounds" | CC0 (kenney.nl page): 100 Ogg files, no attribution needed | Yes, legally; attribution is courtesy | Yes |
| notificationsounds.com | CC BY 4.0: credit with a link back, note modifications; do not rehost the whole library (terms and licence pages) | Yes, with a `NOTICE` credit; one file at a time | Yes |
| Pixabay | Attribution not required; "you cannot sell or distribute Content ... on a Standalone basis" (licence summary) | Doubtful: a sound file in a repository is standalone distribution. Not advised | Link only |
| Freesound | Per file: CC0, CC BY or CC BY-NC (FAQ). NC cannot go in a project that may be sold | Only CC0, checked file by file | Link only, filtered to CC0 |
| Mixkit | Own "Sound Effects Free License"; its terms were not read | Unknown: treat as no | Link only |
| Google Assistant sound library | CC BY 4.0, but the page says distributing the files separately is not permitted (search summary of the page) | No | No |
| Game or film audio (Age of Empires, StarCraft, ...) | Copyright of the publisher | No | Only as Theo's private choice, never in the repository |
| Original sounds made for Torlek | Torlek's own, under the repository licence | Yes | Yes |

This is a reading of the licence summaries, not legal advice; the full licence text governs.

Concrete CC0 candidates, named from a community mirror of the Kenney pack
([`Calinou/kenney-interface-sounds`](https://github.com/Calinou/kenney-interface-sounds), WAV
conversions; the originals are Ogg and the names may differ): `error_001` to `error_008`,
`confirmation_001` to `confirmation_004`, `question_001` to `question_004`, `bong_001`,
`glass_001` to `glass_006`, `pluck_001`, `pluck_002`. Nothing was downloaded or auditioned.

## What makes a notification sound good

| Principle | Source | What it means for five events |
| --- | --- | --- |
| Keep it short; no note under 0.0825 s or it may go unnoticed | Brewster, Wright and Edwards, "Guidelines for the Creation of Earcons" (University of Glasgow; the page gives no year) | About 0.3 to 0.8 s per sound is the working range proposed here; the cited rule only fixes the shortest note. The range is a judgement, not a measurement |
| Loudness is "the main cause of annoyance due to sound" | Same | Level-match all five files; the sound is a nudge, not an alarm |
| Make sounds "subjectively easy to tell apart"; build a family from timbre, register and rhythm first, then pitch and intensity | Same | Give each event its own timbre or rhythm, not just a different pitch |
| Different numbers of notes tell rhythms apart; similar rhythms get confused | Same, citing Patterson (1982) | Use a different note count per event (1, 2, 3 notes) |
| Register works only for relative judgements, and only if registers differ widely; absolute pitch should not carry meaning | Same | Do not rely on "this one is a bit higher" to tell failure from question |
| The more often something happens, the less intrusive its sound should be; use silence on purpose; sounds should feel like one palette | Zafrin and O'Sullivan, Google Material Design feature "Good Vibrations" (no date on the page) | Frequent events get the quietest sound; a rare failure may stand out; all five share a character |
| A simple, light high note rarely startles; bass distorts on small speakers; a sharp-attack accent survives a missed melody; loudness should match importance; related events can share a motif; sound should be optional | Ahva, Futurice, "Please design notifications responsibly (+ free sounds)", 2017-09-05 | Avoid deep bass; per-event on/off already exists |
| Medical alarm standard IEC 60601-1-8 encodes priority by the number of pulses in a burst, with each pulse's fundamental between 150 and 1000 Hz | Trade articles summarizing the standard ([MS Indy](https://mspindy.com/technical-articles/using-audible-alarms-in-medical-equipment-iec-60601-1-8/), undated); the standard itself was not read | An analogy only: Torlek's events are not safety alarms. It supports "count of pulses carries urgency" |

Not found in any source read: a numeric loudness target for desktop notifications, or a
maximum length. The 0.3 to 0.8 s figure and the pitch plan below are proposals.

## Options

Costs are rough guesses for one agent with focused tests, as in the other reports.

### For Torlek's defaults

**Option A: ship nothing, keep the freedesktop names (what #1130 asks for).** An event without
a file plays its theme sound; the document names safe formats (`.oga`, `.wav`).
Pros: no licence question; matches #1130's acceptance; small. Cons: new users keep the
default sounds Theo dislikes.

**Option B: an original set from a checked-in recipe.** One small script, with its parameters,
generates five short WAV files (and their `.oga` forms) that Torlek owns. Distinct timbre,
rhythm and note count per event; level-matched by the same script. Pros: no third-party
licence, no attribution, reproducible, the recipe is greppable. Cons: needs listening and
tuning, which an agent cannot do (this report did not hear any sound); about half a day for
the script and a round of auditions with Theo.

**Option C: copy five Kenney CC0 files with a `NOTICE` line.** Pros: legal at once, about an
hour. Cons: the files were not made as a family and have different levels; the project then
owns 5 binaries it did not design.

### For Theo's own set (kept outside the repository)

**Option 1: pick five from the Kenney CC0 pack.** Audition the candidate names above, copy the
five chosen files to a folder under his home, set the `[notify.sound.files]` paths from #1130.
About 20 minutes once #1130 lands. No licence concern at all.

**Option 2: make five with the Option B recipe.** Same script, Theo tunes the numbers. About
an hour, and the set matches the event character he wants. Depends on Option B existing, or on
running the recipe privately.

**Option 3: game or film clips.** Fun, instantly recognizable, copyright stays with the
publisher; fine as a private choice, never in the repository or a Torlek release.

### Proposed character per event (for Options 2 and B; not auditioned)

| Event | Proposed sound | Reasoning |
| --- | --- | --- |
| finished | One soft two-note rising figure, about 0.3 s, mid register | Quietest and shortest: the sound a user hears most |
| question | Two notes with a rising end, about 0.5 s, a different timbre from finished | A rising contour reads as "asks something"; timbre keeps it apart from finished |
| hold | Three equal short pulses, about 0.6 s | A note count nothing else uses; urgency through pulse count |
| failure | Two falling notes, lower register, about 0.6 s | Falling contour reads as bad news; the only low sound |
| quota | One long soft note with a slow decay, about 0.8 s | One note stands apart from the multi-note events and suits an informational limit |

How often each event fires is unknown here; if finished turns out rare after #1127, its
"quietest" rule should be revisited with the owner's event log.

## Recommendation (proposal)

1. **#1130 as written, Option A, with the two additions below.** No audio goes into the
   repository. The fallback stays the freedesktop name.
2. **For Theo's own sounds now: Option 1.** Audition the Kenney candidates and set five paths.
   It is the fastest path to sounds he likes and carries no licence risk.
3. **File a follow-up issue for Option B only if Theo wants better defaults after living with
   his own set.** Do it after #1130 so it can use the real settings and `torlek notify test`.
   Do not start it from this report.

Additions to #1130's text (proposed, Theo applies them):

> - The user guide names `.oga` and `.wav` as safe formats and says an mp3 depends on the
>   player. (Herdr accepts mp3 only; kitty accepts WAV and OGA on Linux.)
> - A custom sound replaces only the sound: the notifier must not also trigger a terminal bell
>   for the same event.
> - The user guide says no sound ships with Torlek, and links Kenney's CC0 pack as a starting
>   point for owners who want one.

## Decisions for the owner

Proposals only; none is decided.

1. Keep Option A for #1130, or also ship defaults (B or C)? Recommended: A now.
2. For Theo's set, Option 1 (Kenney CC0), 2 (generated) or 3 (game clips)? Recommended: 1.

## Sources, limits and what was not checked

All pages were read on 2026-11-07. A date in the first column is the source's own date where
it gave one.

| Source | Used for | Date |
| --- | --- | --- |
| [Herdr config reference](https://herdr.dev/tui/docs/config-reference/); [repository](https://github.com/herdrdev/herdr) (`src/config/sound.rs`, `src/sound.rs`, `assets/sounds/`, read through the GitHub API) | Keys, file formats, players, embedded files | Sound files added 2026-03-27 (`0403775f`) |
| [Herdr discussion #2529](https://github.com/herdrdev/herdr/discussions/2529) | BEL is a separate path from completion sounds | Read 2026-11-07 |
| [ogadra/dotfiles PR #448](https://github.com/ogadra/dotfiles/pull/448) | A user's done and waiting sounds, five level changes | Opened 2026-11-02, merged 2026-11-03 |
| [OpenCode repository](https://github.com/anomalyco/opencode) (`packages/ui/src/assets/audio`, `packages/tui/src/attention-sounds.node.ts`), [PR #26980](https://github.com/anomalyco/opencode/pull/26980) | Bundled files, per-event defaults, disabled by default | Merged 2026-05-13 |
| [`opencode-plugin-boops`](https://www.pkgstats.com/pkg:opencode-plugin-boops), [`opencode-starcraft`](https://github.com/spartandingo/opencode-starcraft) | Community sound plugins | v2.7.0, no exact date; starcraft undated |
| [Zed repository](https://github.com/zed-industries/zed) (`assets/sounds/`, `docs/src/reference/all-settings.md`, `assets/settings/default.json`), commit `d211f88d` | One shared agent sound, the setting | Sound added 2025-05-27 |
| [Claude Code hooks reference](https://code.claude.com/docs/en/hooks) and [terminal configuration](https://code.claude.com/docs/en/terminal-config) | Notification types, `preferredNotifChannel`, hook example | Undated pages, read 2026-11-07 |
| [alexop.dev post](https://alexop.dev/posts/how-i-added-sound-effects-to-claude-code-with-hooks/); [StaticB1/claude-code-sounds](https://github.com/StaticB1/claude-code-sounds) | Community hook setups | 2026-02-11; repository undated |
| [Codex advanced configuration](https://developers.openai.com/codex/config-advanced) (redirects to learn.chatgpt.com) | `notify`, `tui.notifications`, `notification_method` | Undated |
| [Warp agent notifications](https://docs.warp.dev/agents/capabilities/agent-notifications/) | No sound setting found | Undated; found through search, the page itself was not opened |
| [Ghostty `Config.zig`](https://github.com/ghostty-org/ghostty/blob/main/src/config/Config.zig), [kitty `definition.py`](https://github.com/kovidgoyal/kitty/blob/master/kitty/options/definition.py) (kitty v0.50.0, 2026-11-03), [iTerm2 terminal profile](https://iterm2.com/documentation-preferences-profiles-terminal.html), [iTerm2 triggers](https://iterm2.com/documentation-triggers.html) | Bell options | `main` and `master` as of 2026-11-07 |
| [Arch `sound-theme-freedesktop`](https://archlinux.org/packages/extra/any/sound-theme-freedesktop) | Licence mix | Package 0.8-6, built 2024-07-05 |
| [Kenney Interface Sounds](https://kenney.nl/assets/interface-sounds), [Calinou mirror](https://github.com/Calinou/kenney-interface-sounds) | CC0, file names | Mirror last pushed 2020-12-06 |
| [notificationsounds.com terms](https://notificationsounds.com/terms-of-use) and [licence](https://notificationsounds.com/license) | CC BY 4.0 terms | Undated |
| [Pixabay licence summary](https://pixabay.com/service/license-summary/), [Freesound FAQ](https://freesound.org/help/faq/), [Mixkit licence](https://mixkit.co/license/) | Licence terms | Undated; Mixkit's sound licence text not read |
| [Glasgow earcon guidelines](https://www.dcs.gla.ac.uk/~stephen/earcon_guidelines.shtml) | Length, timbre, rhythm, loudness | Year not on the page |
| [Zafrin and O'Sullivan, Good Vibrations](https://design.google/library/ux-sound-haptic-material-design) | Frequency, hierarchy, palette | Undated |
| [Ahva, Futurice](https://www.futurice.com/blog/designing-notification-sounds) | Pitch, loudness, shared motif | 2017-09-05 |
| [MS Indy on IEC 60601-1-8](https://mspindy.com/technical-articles/using-audible-alarms-in-medical-equipment-iec-60601-1-8/) | Pulse count and pitch range (secondary) | Undated |
| Issues [#1130](https://github.com/Androlax2/agent-cockpit/issues/1130), [#1127](https://github.com/Androlax2/agent-cockpit/issues/1127), [#62](https://github.com/Androlax2/agent-cockpit/issues/62) | Scope | 2026-11-07 |

Limits:

- No audio file was downloaded, played or heard. Every sound judgement above (what is pleasant,
  whether two files are distinct) is untested; Theo must listen before choosing.
- The origin of the audio shipped by Herdr, OpenCode and Zed was not found; absence of a
  stated licence is not a statement that none exists.
- Which formats `pw-play`, `paplay` and `canberra-gtk-play` decode on Theo's machine was not
  tested; the document for #1130 should state them after a test against a fake player and one
  real run.
- The Google Material sound guidance pages and the Edworthy alarm-design paper could not be
  fetched; the first is represented by the Google Design feature, the second by trade articles.
- Warp, Slack and iTerm2 custom-bell behaviour rests on documentation pages or search
  summaries, not on running the products.
- Licence summaries are not legal advice.
