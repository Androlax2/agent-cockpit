---
version: alpha
name: Agent Cockpit terminal design
description: Approved cost-reporting and desktop team-visibility handoffs for the Rust terminal cockpit.
colors:
  background: "#1a1b26"
  foreground: "#c0caf5"
  secondary: "#a9b1d6"
  selection: "#414868"
  blue: "#7aa2f7"
  cyan: "#7dcfff"
  green: "#9ece6a"
  amber: "#e0af68"
  red: "#f7768e"
  purple: "#bb9af7"
typography:
  body:
    fontFamily: JetBrainsMono Nerd Font
    fontSize: 12px
    lineHeight: 1.5
    fontWeight: 400
  heading:
    fontFamily: JetBrainsMono Nerd Font
    fontSize: 14px
    lineHeight: 1.2857142857142858
    fontWeight: 700
---

## Cost reporting

### Overview

The terminal cockpit separates API-credit usage estimates, subscription API-equivalent value, and usage whose billing mode cannot be verified. Implement the approved Option A without redesigning the cockpit or implying that token-based estimates are a provider bill.

### Colors

Reuse the existing Rust terminal palette and style helpers in `apps/cockpit/src/app.rs`: `MAGENTA` for monetary amounts, `CYAN` for active time, `FG` for labels, `FG_DARK` for secondary text, and `YELLOW` for existing error messages. Keep quota colors unchanged. Category meaning must remain explicit in text, not color alone.

Do not translate ANSI palette indices into invented RGB tokens or introduce a separate CSS token system.

### Typography

Use the terminal's existing font, cell measurements, `width_of`, money formatter, and bold/dim styles. FigCraft font sizes and pixel dimensions illustrate the layout; they are not new terminal typography tokens.

### Layout

Keep the current single-line header, quota priority, clock placement, Status navigation, and spending-section placement before service health. Preserve the existing `TODAY`, `WEEK`, and `MONTH` periods and the `Active time` row's calculation and conditional visibility.

Compute column widths from the actual category labels and formatted amounts. Retain the existing stacked-period fallback when columns do not fit. Wrap explanations and long labels rather than clipping their billing meaning. Hide the header monetary fragment when it cannot fit intact; do not displace quotas to show it.

### Components

#### Approved FigCraft handoff

Use `design-plans/cost-reporting-split.figcraft`. Theo approved Option A; frames `2` (header and wide Status table) and `4` (narrow stacked fallback) are marked `readyForDev`. Frame `5` contains the scope and edge-state notes. Frame `3` is the rejected combined-value header and must not be implemented.

Reuse the existing Rust header and Status renderers in `apps/cockpit/src/app.rs`. No new component framework is required.

#### Header

Show `API ≈ <amount> today`, for example `API ≈ $12.00 today`, using only usage attributed to API credits by verified billing evidence. Never add subscription or unknown-billing values to this amount.

For historical-only or otherwise unverified billing, omit the header dollar fragment rather than presenting the old combined value or implying a verified zero. Update the existing header help entry to describe the API-credit estimate and direct users to Status for the other categories.

#### Status spending

Use this exact section caption:

`SPENDING · token-based estimates, not your bill`

Replace the combined Cost row with these mutually exclusive rows, in this order:

- `API credits (est.)`
- `Subscription (API value)`
- `Unknown billing (API value)`

Keep all three rows when recorded usage exists, including zero-valued categories. Amounts remain token-based estimates using the existing money formatter; an API-credit estimate is not an actual credit balance or provider charge.

Use these explanations below the amounts, wrapping to available space:

- `Subscription = API-price value, not a subscription charge.`
- `Unknown = billing mode unverified; includes historical records.`

Rename `All-time cost` to `All-time API value`, retaining its aggregate scope across all three categories and showing `≈` before its amount. It is not an all-time API-credit spend total.

#### Evidence and edge states

Classify billing from verified record-specific evidence. Historical or unverifiable records stay in Unknown; do not infer their billing mode from the current session, provider, or subscription settings. A record can have a known token-price estimate and still have unknown billing.

Distinguish zero recorded estimated value from missing data. With no recorded usage, state `No recorded usage` instead of fabricating a verified zero bill. Loading and archive failures must not become dollar-zero values; preserve explicit error handling and visible archive errors.

If some records cannot be priced, show a visible incomplete-estimate warning, with a count when available. Unknown token pricing and unknown billing mode are different conditions; neither may silently disappear into an apparently complete total.

#### Verification handoff

Extend the existing Rust `Fixture`, `texts`, and `render` tests in `apps/cockpit/src/tests.rs`, including the spending-placement and archive-error tests. Reuse `apps/cockpit/tests/costs.rs` for billing aggregation fixtures rather than adding browser or Storybook tests.

Cover mixed billing, historical-only billing, zero-valued categories, no recorded usage, unpriced records, archive errors, large formatted amounts, and wide/narrow layouts. Assert the header excludes subscription and unknown values, the exact category labels remain readable, amounts do not overlap, and Active time and service-health ordering are preserved.

### Do's and Don'ts

- Keep implementation limited to the approved cost-reporting split and its existing terminal rendering/test patterns.
- Do not call estimates actual spend, paid charges, invoices, or provider balances.
- Do not add browser pages, Storybook, dependencies, invoice integration, or new provider-billing fetches.
- Do not modify unrelated product UI or replace the existing terminal design system.
## Desktop team visibility

### Overview

The desktop cockpit makes task-linked sessions and handoffs visible inside the existing project list, without replacing the session terminal or introducing a separate dashboard.

### Colors

Reuse the terminal's ANSI color roles rather than hard-coding RGB colors in Ratatui. Use cyan for task grouping, blue for working activity, amber for explicit attention, and secondary text for supporting context. Keep existing project colors for project headings.

Selected member rows use foreground text throughout on the selection background; secondary text loses sufficient contrast on that surface. Pair color with status words, disclosure markers, and the selected-session rail.

### Typography

Use the existing terminal font. The typography tokens describe the reference canvas, not a request to change Kitty's font settings. Render headings using terminal emphasis, not a second font.

### Layout

Keep the existing project groups, conversation split, pull-request panel and todo panel. Nest linked sessions beneath their shared task, with independent sessions following the teams in that project.

Use terminal-aligned tree rails and compact lines instead of rounded cards. Show role and state, session identity, and latest reported activity together. Keep handoff context in the selected-session details.

Collapse teams without hiding their attention reason. Preserve the existing left/right terminal-focus shortcuts; Enter on a team toggles its disclosure and Enter on a member opens that session.

### Components

Use the approved member-row states for ordinary, selected, and attention-needed sessions. A planned reviewer is visibly distinct from a live linked session and cannot open a terminal.

Keep session names on one ellipsized line when space is constrained, with the full identity available in selected-session details. Preserve readable waiting reasons and activity age.

Use explicit empty, loading and unavailable notices rather than presenting unknown relationships as independent sessions. Missing activity reads as missing; a finished turn is not a completed task.

### Do's and Don'ts

- Do preserve the current session and keyboard focus when team information refreshes.
- Do name the blocker and the person or role needed next when evidence exists.
- Do keep QA review, application and deployment as separate states.
- Don't infer team membership from similar titles, matching directories or visual proximity.
- Don't modify the terminal configuration to implement this feature.

## Cockpit UI redesign (desktop column and phone)

### Overview

Todo `014f67c1`. Theo (ADHD) gets lost among sessions on both the desktop column and the phone. Every
session must answer, in this order: **what it is doing now, why (project › task), and does it need
me**. Approved in FigCraft: `design-plans/cockpit-session-list.figcraft`. The Notes text on page
`01 Wireframes` holds the brief, every decision and its reason. PNG previews of the approved
visuals: `design-plans/previews/1..9-*.png`.

Pages: `01 Wireframes` (session card, layout A, edge states 1-14 in frame `232`), `02 Visual`
(approved colour design), `03 Navigation`, `04 Open session`, `05 Chief`, `06 Approvals`
(wireframes that define behaviour). Every frame meant for building is marked `readyForDev`
(`inspect.readyForDev`). `Desktop B · by project (rejected)` must not be built.

### Colors

Tokens: `design/tokens.cockpit-ui.{json,css}` (collection `Cockpit`). Same Tokyo Night values as
`app.rs` and the phone's `page.html` (`--card`, `--dim`, `--line` come from the phone). In the
terminal, use the ANSI roles, not RGB.

- **Amber means "needs you" and nothing else**: the card rail and action bar, NEEDS YOU heading,
  the Sessions badge, the primary key cap / Approve button, Chief escalations.
- Blue: working / running. Green: done, healthy. Red: warnings (changed build files, hidden
  characters), failed. Purple: ideas, money estimates, the Chief's suggestions.
- **Remove `YELLOW` from `PROJECT_COLORS`** (keep MAGENTA, CYAN, GREEN, BLUE): a yellow project
  name reads as "needs you". Project names keep their hashed colour everywhere.

### Typography

Desktop: the terminal font, bold for the "doing now" line and headings. Phone: system-ui (Noto Sans
in the canvas), 16/15 body, 13-14 secondary, as today.

### Layout

**Desktop column.** One header line, tabs, one view at full height:

- Header: one chip per usable plan with a 4-cell bar and % (`Claude Max ▮▮▯▯ 34%  Codex ▮▯▯▯ 12%`;
  a plan running low turns red), then `≈ $73 today` = API-price value of **all** usage today
  (subscription + API + unknown), then the clock. This supersedes the Cost-reporting header rule
  (`API ≈ $ today` only), by Theo's decision: he wants the estimate even on subscription. Status
  keeps the three-row split and adds the total.
- Tabs `[1] Sessions (badge) · [2] Todo (badge) · [3] Ideas (badge) · [4] Teams · [5] Chief`
  (Round 4); `s` opens Status over the
  column (usage, spending, Chief, pull requests, services). Pull requests move into Status.
- Proposed todos move out of Todo into Ideas (one place). Done today folds (`d`).
- Every key is drawn as a key cap (selection background, bold key); footers list the view's keys.

**Session list (Sessions tab).** Grouped by attention, across projects: NEEDS YOU, WORKING, QUIET
(finished, asleep, the Chief). Card = line 1 status glyph (fixed width) + **doing now** + age;
line 2 `project › task` aligned under the title (the task truncates, never the project); line 3
only when it needs Theo: an amber bar with the exact ask and its key. Selected card: selection
background. Phone: same groups, QUIET folded into one "Show ›" row, 44 px action button.

**Opening a session.** The ask is pinned above the agent's terminal (desktop) / at the top of the
chat (phone); conversation below. After answering, offer the next session that needs Theo
(`⏎ go there / esc stay`; phone: Go there / Stay). Not needing Theo: a one-line strip.

**Approvals (ask-to-run), Theo's option B.** Approve in the pinned box with `y` (`n` decline, `i`
details; Enter does nothing). No focus-stealing window. The trusted terminal opens below only for
sudo/prompts and is never replaced while Theo types. States: waiting (sudo marked, warnings, hidden
characters shown as `⟨U+200B⟩`), password, running (`▶ Running your approved command · 3m of about
10 min`, last output lines), done/failed (exit, duration, offer next), timed out (`nothing ran`,
`⏎ ask again`). Covers todos `00caa7a7`, `2825b39e`, `3a831b7b`, `6b636c5f`, `35d19c18`.

**Chief** (spec `docs/superpowers/specs/2026-10-24-autonomous-chief-design.md`): escalation pinned
(`Chief needs you · Security|Scope|Not sure`, the agent's question, why, `◆ Chief suggests`; `a`
use suggestion, `w` write own); `Chief ✓N` on cards → answers list (`c`), overrule (`o`) with a
preview of the habit it creates; habits list (`x` delete, `p` project); the 5 Chief tab (Round 4).

### Components

- Session card (`Session card / Desktop`, `/ Phone`): variants Needs you, Working, Quiet.
- Edge states (frame `232`): empty list; two asks (one bar, `1 of 2`, oldest first); stale status
  (`no update 18m`, falls back to the last tool); long text; **teams are never split** (one block,
  in the highest group any member needs, member lines as built in Desktop team visibility); Chief
  escalation; `Chief ✓2`; links `⇄1`; waiting on a todo (`↳ Waiting on …`, queued); Chief card and
  "Chief is off" banner; approval waiting (sudo) vs approved command running; approval timed out;
  Git/GitHub read-only line (`R restart`); session contained (relaunch); GitHub outcome unknown.

**Away from the cockpit** (page `07`, preview 10). A top-centre banner appears only when a
session needs Theo (no "finished" banners). Several at once merge into one banner with the count
and the oldest ask. Clicking it focuses workspace 6 with that session selected and its pinned ask
open. Banners stay held during meetings and games, as today. The waybar chip shows `◐ N` (working,
nothing for Theo), `! N` (needs you) or nothing. Phone pushes follow the same rule; a tap opens the
session's pinned ask.

**Finding and starting** (page `08`, preview 11). There is no search. `:` goes away and each of its
actions moves to a card, to session details (`i`) or to Status; the attention filter, queue and
updates are replaced by the grouping. `?` shows the current tab's keys, the global keys and a legend
of the signs. Most work starts from a todo (`r`). `N` starts a blank session: project (recent
first), agent, first message, own clone. `i` shows why, where and who, what is around the session
(Chief answers, links, queued messages), Theo's private note, and the session's actions. `l`
(later) hides an ask for 20 minutes: it waits in QUIET, comes back by itself, and never approves.

**Phone, remaining screens** (`02 Visual`, preview 12): Todo, Status (with a This phone card), New
session, Chief escalation.

**Smaller windows** (page `09`, preview 13):

- Guide panel: step progress bar, the session it comes from, each link or value in a box with its
  key, the agent's question above the answer box, `⌘S I'm stuck` always shown.
- Todo form: a card instead of a black full-screen overlay, the kind as chips.
- Settings: grouped cards, a one-line explanation per row, on/off pills.
- "Still there?": a big amber countdown, plus which sessions keep working and what needs Theo.

**Focus aids** (a "back from a break" recap, a single-task view, a digest screen, quiet hours): Theo
does not want them for now. Don't build them.

### Round 3: the remaining surfaces

These are on page `10 Remaining surfaces` (previews 14-17), every frame `readyForDev`.

- **Notices** (frame `2081`): one line each, under the tabs, with a coloured left edge and never
  amber. Examples: a Claude incident (red, `o status`), sessions paused while gaming (blue), disk
  full (red), sessions that ran before a restart (blue, `B restore`). At most 2 show, then `+N more
  · s for Status`. **greyproxy holds become a NEEDS YOU card** ("Allow or deny in greyproxy",
  `G review`), because only Theo can decide them.
- **Badges:** at most 2 on line 1, before the age, most urgent first: `⟳` restart needed, `↑N`
  unpushed, `✉N` queued, `⬆` update, `$` on API credits. The selected card adds a meta line with
  agent, model, effort, ctx %, `◷` background agents, `⇅` remote, billing. Model, effort, ctx,
  `◷`, `⇅`, `▭` and `◇`/`◆` never sit on line 1.
- **Missing states:**
  - restarting: `⟳ Restarting…`, in WORKING;
  - unseen result: `● Finished · new result`, in QUIET, green dot and bright text until opened;
  - idle: `○ Idle 25m · waiting for your next message`;
  - blocked: `↳ Blocked: its todo isn't running · r run it`;
  - unknown: `· State unknown (no report for 10m)`.
- **Session details `i`** (frame `2200`): where and who (clone, branch, unpushed, agent, model,
  effort, ctx, since, billing, ≈ cost), what is around the session, Theo's note, then every action
  grouped:
  - Work: `e` `$` `C` `c` `k` `d`;
  - Code: `g` `U` `+` `o` `t`;
  - Session: `m` `w` `z` `R` `x`.

  The same keys work on the selected card.
- **Chief main view** (`s` → Chief, frame `2284`; superseded by the 5 Chief tab, Round 4):
  - On/off with today's numbers and the session it runs as;
  - keys: `c` pause/resume, `p` pick another chief, `r` summary now, `l` log, `h` habits;
  - uncertain deliveries are NEEDS YOU rows (`y` it arrived / `x` stop safely);
  - the chief picker shows readable names and says in words why a row can't be chief
    (todo `0b6cba5b`).
- **Team view** (`i` on a team, frame `2335`; superseded by the 4 Teams tab, Round 4):
  - members with role, activity and state;
  - link requests (`y` / `n` / `e` edit purpose) and failed starts or deliveries (`d` retry /
    `c` cancel) as NEEDS YOU rows;
  - history; `u` unlink.
- **Todo extras** (frame `2390`):
  - row columns: priority marks, glyph, kind, title, queue place `Q1`, project; `◇` marks a
    GitHub issue;
  - the details card shows kind, priority, queue and source, with keys `r` run now, `e`, `+ -`,
    `Q`, `K`, `m`, `h`, `d`, `x`;
  - `f` filters by kind and `D` shows done today. Todo's status tabs fold into these sections.
- **Subscription fallback and cost** (frame `2475`):
  - an exhausted plan turns its header chip red;
  - a switched session reads `⇆ Switched to Codex · back on Claude at 18:00` (WORKING); with no
    plan left it reads `‖ Paused: no plan left · resumes 18:00 by itself` (QUIET; `$` switches it
    to API credits);
  - Status → Spending adds "today by session ≈ at API prices" with the waste flags (`⚠ 3 long
    retries`).
- **Phone extras** (frames `2498`, `2524`, `2550`):
  - a "This session" sheet: tell me when it finishes, show the terminal (read-only), Remote
    Control on/off, Interrupt (tap twice, red outline);
  - a permission dialog as a pinned card with Yes/No ("other choices: answer on the computer");
  - a command question (`[y/N]`) as a pinned card ("Yes (tap twice)", never a password);
  - pairing with 6 digit boxes, the time left, tries, and Pair;
  - Wake stays a button on an asleep card.

### Round 4: Teams and Chief tabs

Todo `03f69165`. Page `11 Teams & Chief tabs (wireframe)` (brief, Theo's answers and sources in
its Notes frame), page `12 Teams & Chief tabs (visual)`, previews 18-19. Ready for dev on page 12: `3409`, `3670`,
`3553`, `3737`, `3803`, `3811`, `3973`. They supersede page 10 frames `2284` (Chief main view) and
`2335` (Team view), and page 05 frame `1128` (Status → Chief), which are no longer ready for dev.

- **Tabs** `[1] Sessions · [2] Todo · [3] Ideas · [4] Teams · [5] Chief` (frame `3811`). Teams
  and Chief badges: the needs-you count in amber; otherwise Teams shows the number of live teams
  (plain) and Chief shows `●` on (green) or `○` off. To fit 60 cells, tabs drop the extra cell
  before and after each one; when the row still overflows, inactive tabs fold to key cap + badge,
  from the right, and the open tab never folds. Names come back as soon as they fit.
- **4 Teams** (frame `3409`): list on top, the selected team below, nothing to open. List groups:
  NEEDS YOU, WORKING, then `▸ Ended today · N` folded (`D`). Team card: glyph, task, badges, age;
  line 2 `project › task · Role ● Role ○`; the amber bar names the ask. Below the divider, the
  selected team: MEMBERS (role, activity, state), NEEDS YOU (link requests, failed starts,
  failed deliveries), IN FLIGHT (queued, unread, cancelled messages and handoffs), RECENT (the
  last 3 events, including restarts that keep, lose or recover a membership).
- **Inside a team** (`⏎`, frame `3670`): `j` `k` move across members and rows, `⏎` on a member
  opens its session, `y` `n` `e` / `d` `c` act on the selected needs-you row, `l` full log, `u`
  unlink, `esc` back to the list. These keys do nothing from the team list.
- **5 Chief** (frame `3553`): the state card (on/off, today's numbers, the session it runs as),
  keys `c` pause/resume, `p` pick another chief, `r` summary now, `l` answer log, `h` habits;
  NEEDS YOU (uncertain deliveries, `y` / `x` / `i`); NOW (what it is deciding or waiting for);
  TODAY (answers, escalations, pauses; `⏎` opens that session, `o` overrules an answer).
- **Sessions** (frame `3737`): the team block stays (teams are never split) but keeps only role,
  doing now and age per member, plus `5 team details`. `i` on a team opens 5 with that team
  selected. **Status** keeps one Chief line pointing to 5 (frame `3803`).
- Member states: live (blue `●`), asleep (`◌`, dim), gone (red `✕`, with `⏎ re-attach`), planned
  (`○`, dim). Team names are cyan. Selected rows use foreground text on the selection background.
- Edge states (frame `3973`): no teams, team data unavailable (red, `R retry`, never shown as
  independent sessions), loading, long task names, ended team, Chief off, no chief yet, a quiet day.
- Not in this round: the phone versions of both tabs.

**Rule conflicts, settled by Theo:**

- Keep "Tell me when it finishes" (a phone opt-in per session), the daily digest, and the
  80%/95% usage alerts.
- Remove the OSD "Finished" banners, because only needs-you interrupts.

**My calls:**

- Amber: greyproxy holds become needs-you. Disk and incident notices use red. Pins, `$`,
  `/compact`, `⟳` and `↑` use dim or blue. Footer messages use foreground and confirmations blue.
  The header's "N need you" becomes amber (it was red).
- Waybar keeps a dim glyph when nothing runs, because its script must not be empty.
- The README key list will be rewritten from the key map.

### Key map

One key map for the whole column. It resolves every clash between the redesign and today's keys
(`app.rs` `App::handle`). Keys work in the focused place only: **in the column, `y`/`n` never
answer anything.** Approving happens in the pinned ask once the session is open (`⏎` or `→`).

| Key | Where | Does | Was |
|---|---|---|---|
| `1` `2` `3` `4` `5` | everywhere (also CMD+1…5 from the session) | Sessions, Todo, Ideas, Teams, Chief tabs | 0-4 panels; F1-F4 and Tab panel cycling go |
| `s` | everywhere | Status overlay (usage, spending, Chief, PRs, phone, services) | `s` slept the session: now `z` |
| `?` | everywhere | help for the current tab + legend | same |
| `esc` | everywhere | close / back | same |
| `n` | Sessions | next session that needs you | same |
| `⏎` / `→` | Sessions | open the session (focus its pane and pinned ask) | same; `l` no longer does this |
| `L` | Sessions | later: hide this ask 20 min, never approves | new (`l` stays vim-right inside lists) |
| `i` | Sessions, Todo, Ideas | details of the selected item | Todo `i` "in progress" moves into Todo details |
| `p` | Sessions | pin: pinned cards come first **inside their group**, marked `⚲` | same key; pins no longer jump above NEEDS YOU |
| `z` | Sessions | put the session to sleep | was `s` |
| `R` | Sessions | restart the session (confirm) | was `r`; CMD+R (restart all stale) stays |
| `x` | Sessions | end the session (confirm) | same |
| `e` `$` `C` `g` `U` `+` | Sessions | effort, billing, /compact, lazygit, push/apply, parallel clone | same; also listed in details `i` |
| `o` | Sessions | open the session's design canvas | same (Chief overrule `o` exists only in the Chief answers list) |
| `N` | everywhere | new blank session: project, **agent**, first message | `a` and `O` (agent pick) fold into `N`; Todo's `N` is now `r` |
| `H` | everywhere | resume a past session (the existing picker) | same |
| `B` | Sessions, after a reboot | restore the sessions that ran before | same |
| `P` `W` `G` | everywhere | PR, Claude web, greyproxy windows | same |
| `S` | everywhere | Settings | same |
| `t` / `e` | Todo | add / edit (the form) | same |
| `r` | Todo | run now: start a session on it in a clone | was `N` |
| `d` | Todo | done / back to To do | same; folding "Done today" is `D` |
| `f` `K` `m` `Q` `+` `-` `h` | Todo | filter, kind, move, queue, priority, hand over | same, listed in Todo help |
| `y` / `n` | Ideas | yes, add to To do / no | same |
| `y` / `n` / `i` | pinned ask (session focused) | approve / decline / details | approval pane keys move here |
| `a` / `w` | pinned Chief escalation | use the suggestion / write my own | `a` no longer starts sessions |
| `⏎` / `o` | Status → pull requests | start a review session / open on GitHub | `c` in the old PR panel |
| `c` `p` `r` `l` `h` | 5 Chief tab | pause/resume, pick chief, summary, answer log, habits | new; was Status → Chief |
| `⏎` / `esc` | 4 Teams tab | into the selected team / back to the list | new |
| `c` | Sessions card with `Chief ✓N` | that session's Chief answers | new |
| `/` and `:` | — | removed (Theo: no search, better help) | `/` find, `:` palette |

### Build dependencies and open points

- **"Doing now" does not exist yet.** Theo chose: agents publish a short per-step status (a
  command like `cockpit-session-label`), falling back to the last tool from the OpenCode plugin /
  Claude hook when stale. The card needs it; build it first.
- Approving in the column makes it an approval surface: **security review before building**
  (the phone already is one).
- Chief, team, link and broker states depend on their own work (`b77a0881`, the Git broker
  rollout); build their card slots, hide them until their data exists.
- "Ask again" after a timed-out approval and the GitHub-broker states need machine-readable
  sources (not printed text).

### Do's and Don'ts

- Do keep amber for "needs you" only, and pair every colour with a word or glyph.
- Do show every key as a key cap; Enter never approves.
- Don't split a team across groups; don't hide a project name to fit a task.
- Don't build the rejected layout B or put anything back behind `:` that has a place on a card.
