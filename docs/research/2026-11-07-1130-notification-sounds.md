# Making Torlek's own notification sounds: a sound-design guide

For whoever builds Torlek's own notification sounds (Theo first, then the issue that follows
[#1130](https://github.com/Androlax2/agent-cockpit/issues/1130)). Research for #1130
(milestone v0.1.0-alpha.7, owner request from Theo, 2026-11-07: "I'd like to create custom
sounds for each notification, I don't really like the default one"). It follows the layout of
the other reports in this folder.

**Research and proposal.** Merging this document accepts research delivery only: it adds no
audio file, no product code and no dependency, it does not implement #1130 (which waits on
[#1127](https://github.com/Androlax2/agent-cockpit/issues/1127)) and it leaves #1130 open.
Every recommendation below is a proposal for Theo to accept or change.

**Scope, as Theo clarified it on 2026-11-07** (relayed by the coordinator session after the
first version of this report): the goal is not to copy, ship or recommend other projects'
sound files. It is to learn how other tools got or made their sounds, so that Torlek can
create its own. The report is therefore a guide: techniques with sources, then concrete
recipes Torlek can synthesize itself.

## The question

Today each notification event plays a fixed freedesktop sound name from the owner's sound
theme (#1130 lists `dialog-error`, `dialog-warning`, `message-new-instant`,
`alarm-clock-elapsed`, `complete`). The events are failure, hold, question, quota and
finished. Theo dislikes the theme sounds. What would five original sounds look like, how are
such sounds designed, and how does each stay distinct from the others?

Read in code (`crates/cockpit-notify` README and `src/outlets.rs`, base commit `1a7d869b`):
the notifier plays one sound per batch of events, the most urgent event's, and never while a
hold stands. It plays by freedesktop name with `canberra-gtk-play --id=NAME` when there is a
display, otherwise `pw-play` or `paplay` plays `NAME.oga` from the theme's `stereo` folder.
`failure` is raised when a session failed or its turn failed, `hold` when a command waits for
approval or a permission dialog is open, `question` when the agent asks one, `quota` when the
usage limit stopped a turn, and `finished` when the task list finished or a turn completed.
The crate has no terminal-bell code.

## How the surveyed tools got their sounds

Read on 2026-11-07. The finding is mostly an absence: almost no project says how its sounds
were made.

| Tool | What it plays | How the sounds were made or sourced |
| --- | --- | --- |
| Herdr | Two embedded mp3 files, `done` and `request` (34,560 and 46,811 bytes), added 2026-03-27 | Not stated. Owners can replace them (`ui.sound.path`, `done_path`, `request_path`; mp3 only). One user's dotfiles lowered the same two files with ffmpeg five times before the level felt right |
| OpenCode | 45 embedded sounds in five families (`alert`, `bip-bop`, `nope`, `staplebops`, `yup`); defaults: `bip-bop-01` general, `bip-bop-03` question, `staplebops-06` permission, `nope-03` error, `yup-01` subagent done | Not stated. The family names show a vocabulary: a positive, a negative, a neutral, a "needs you" and an alert character, each with several variants. Ships disabled in v1 (2026-05-13) |
| Zed | One embedded wav, `agent_done`, for both "done" and "input required" (added 2025-05-27) | Not stated. The owner can only choose `never`, `when_hidden` or `always` |
| Claude Code, Codex | Nothing of their own; terminal bell, desktop notification, or a hook that runs a player | Owners run a system sound (`afplay .../Glass.aiff`) or a downloaded file |
| Warp | Toasts and desktop alerts; no sound setting found in its documentation | Not applicable |
| Ghostty, kitty, iTerm2 | The terminal bell: the system sound, or an owner file (`bell-audio-path`, `bell_path`) | The owner's file or the system theme |
| Community hook setups | Game voice lines (Age of Empires, StarCraft), a stock wav, 448 stock sounds from a notification-sound site, macOS system sounds | Taken from games, stock sites or the OS. The StarCraft plugin downloads the clips on first run and ships none |

What the survey does tell Torlek:

- **Torlek's five events are finer than any surveyed tool.** Herdr keys two files, OpenCode five
  built-in ids, Zed one sound. Original sounds would have to be made for a finer split.
- **The two well-documented cases of how sounds were made are Google's, not a terminal
  tool's.** Zafrin and O'Sullivan describe recording two piano notes an octave apart for a
  brand sound, building progress cues from stacked chords, and reusing one key across the
  family. Section "Techniques" builds on them.
- **Level matching is where hand-made sets cost time.** Five amplitude changes on two files
  (above) is the cost of mixing sounds that were not made together.
- **A second sound source can double the beep.** A Herdr discussion notes the terminal bell is
  a separate path from the agent sounds, so muting one leaves the other. Claude Code's
  `terminal_bell` and Codex's `bel` method are such paths. Torlek's notifier rings no bell
  (checked in code), so the double would come from the agent's own settings.
- **Herdr falls back, #1130 refuses.** Herdr reports a non-mp3 path and plays the default
  sound. #1130 asks for a refusal at configuration load, which is the stricter rule in the
  user's global guidance ("fail loud"). Keep #1130 as written.

## Techniques, with sources

| Technique | What to do | Source |
| --- | --- | --- |
| Keep it short | Short sounds keep up with the interface; no single note under 0.0825 s or it may not be noticed | Brewster, Wright and Edwards, "Guidelines for the Creation of Earcons" (Glasgow; no year on the page) |
| Treat loudness as the main risk | Intensity is "the main cause of annoyance due to sound"; level-match every sound; the more often an event fires, the quieter its sound | Same; Zafrin and O'Sullivan, Google design feature "Good Vibrations" (undated) |
| Separate sounds by timbre first, then rhythm and register | Choose timbres that are "subjectively easy to tell apart"; start a family from timbre, register and rhythm, then refine with pitch and intensity | Brewster et al. |
| Use rhythm and note count, not pitch alone | Different note counts tell rhythms apart; similar rhythms are confused (Patterson 1982, cited by Brewster); register helps only if the registers differ widely | Brewster et al. |
| Shape the envelope | An envelope is attack, sustain and decay. A longer attack gives a smoother start; "decay is the fade-out time". A sharp-attack accent keeps a sound audible if the melody is missed | sfxr readme (DrPetter, 2007-12-14); Ahva, Futurice, 2017-09-05 |
| Pick the waveform for the character | sfxr offers a small set of base waveforms (square by default, also sawtooth, sine, noise). A sine is soft, a square or sawtooth is buzzy; adding partials to a sine makes bell, pluck or woodblock timbres | sfxr readme; pyfxr documentation (undated) |
| Choose intervals relative to a key | Treating a melody as a bag of intervals had no predictive power; intervals counted relative to the key predicted happy versus sad passages with 85% accuracy. So make "resolved" and "unresolved" by which scale degree a sound ends on, not by an isolated interval | Kaygusuz and Zuluaga, "Impact of Intervals on the Emotional Effect in Western Music", arXiv 2018-12-10. A 2023 summary on musicscience.net notes automatic effects may be weaker than self-reports |
| Stack for progress | Share a first note, add notes or chords as the state advances (Google: a two-note, a three-note and a four-note chord as a subject nears the centre; Futurice: the same first note, a tritone for the final stage) | Google design feature; Ahva |
| Use one key as the palette | Let variations differ but keep one system: Google reuses a G-based key across newer sounds "like a color palette" | Google design feature |
| Add a little warmth | A tiny gap between two close notes gives a "flamming" playfulness (Google); a slight detune between two copies of a note gives a slow beat | Google design feature; the beat is the author's addition, to audition |
| Avoid the extremes | Bass distorts on small speakers; a simple, light high note rarely startles | Ahva |
| Encode priority by pulse count | Medical alarm standard IEC 60601-1-8 separates priorities by the number of pulses in a burst, each pulse with a fundamental between 150 and 1000 Hz. An analogy only: Torlek's events are not safety alarms | MS Indy summary (undated); the standard itself was not read |
| Use silence on purpose | Not every event needs a sound; too many dilute the ones that matter | Google design feature |

Not found in any source read: a numeric loudness target or a maximum length for a desktop
notification sound. The lengths and levels below are proposals.

### Tools that can make these sounds on the host

Checked on this host on 2026-11-07: `ffmpeg` n9.0.2, `python3` with numpy 2.5.4, and the
libvorbis encoder. `sox` is not installed.

- **ffmpeg** has a `sine` source (`frequency`, `duration`, `sample_rate`), an `aevalsrc`
  expression source, `afade` (`type`, `duration`, curves such as `exp` and `qsin`), `adelay`
  and `loudnorm`. It is good for one-note sounds and for converting to `.oga`.
- **numpy plus the standard `wave` module** gives full control over partials, envelopes and
  sequences; the sketch below uses only those.
- **sfxr and its ports** (DrPetter's sfxr is MIT-licensed, by its readme) make chip-style
  sounds from a few sliders. They suit blips, not bell or woodblock timbres; mentioned only
  because several game-sound tools follow its envelope vocabulary.

## Keeping five events distinct

The proposal gives each event its own rhythm and timbre, shares one key (D major) so the five
sound like one family, and uses the scale degree where a sound ends to say "resolved" or
"waiting".

| Event | Notes and rhythm | Timbre | Ends on | Length | Why |
| --- | --- | --- | --- | --- | --- |
| finished | One note, D5 | Bell-like (inharmonic partials 1, 2.76, 5.4: the author's choice, to audition) | The tonic (D): resolved | 0.30 s | The calmest, shortest sound: nothing is wrong |
| question | Two notes, short then long, A4 then E5, rising | Plucked (harmonic partials 1 to 4) | The 2nd degree (E): unresolved | 0.46 s | A rising, open ending asks for an answer |
| hold | Three equal 0.09 s pulses, G5 | Dry, woodblock-like | A single repeated pitch | 0.45 s | A note count no other event uses: urgency through pulse count |
| failure | Two falling notes, D4 then B3, a minor third down | Triangle wave (alternating-sign odd harmonics), dull | The 6th degree (B): falls away | 0.54 s | The only low, falling sound |
| quota | One long note, A4, slow attack | Soft sine with a slightly detuned copy | The 5th degree (A): neutral | 0.80 s | A long soft single note for an informational limit |

The weakest pair is finished and quota: both are single notes, told apart by length, attack
and timbre. Audition that pair first. How often each event fires is unknown here; the rule
"frequent events are quietest" should be checked against the owner's event log once #1127
lands.

## A recipe Torlek can run

The sketch below makes all five WAV files. It was run on 2026-11-07 into a scratch folder
outside the repository (nothing was committed) with Python and numpy 2.5.4, and measured with
ffprobe and ffmpeg `astats`. The audio was not heard: an agent cannot listen, so Theo must
audition it.

```python
"""Sketch: synthesize five notification sounds with numpy and the stdlib wave module."""
import sys
import wave
from pathlib import Path

import numpy as np

SAMPLE_RATE = 44100
TARGET_RMS_DBFS = -22.0
GAIN_OFFSET_DB = {"finished": -3.0}
PEAK_CEILING_DBFS = -3.0


def midi_to_hz(midi_note):
    return 440.0 * 2 ** ((midi_note - 69) / 12)


def note(midi_note, seconds, partials, decay_seconds, attack_seconds=0.005):
    time = np.linspace(0, seconds, int(seconds * SAMPLE_RATE), endpoint=False)
    fundamental = midi_to_hz(midi_note)
    wave_sum = sum(
        weight * np.sin(2 * np.pi * fundamental * ratio * time) for ratio, weight in partials
    )
    envelope = np.exp(-time / decay_seconds) * np.minimum(1.0, time / attack_seconds)
    tail = np.minimum(1.0, (seconds - time) / 0.01)
    return wave_sum * envelope * tail


def sequence(*parts):
    return np.concatenate(parts)


def silence(seconds):
    return np.zeros(int(seconds * SAMPLE_RATE))


BELL = [(1, 1.0), (2.76, 0.4), (5.4, 0.2)]
PLUCK = [(1, 1.0), (2, 0.5), (3, 0.3), (4, 0.15)]
WOODBLOCK = [(1, 1.0), (2.4, 0.5)]
TRIANGLE = [(1, 1.0), (3, -1 / 9), (5, 1 / 25), (7, -1 / 49)]
DETUNED_SOFT = [(1, 1.0), (1.007, 0.7), (2, 0.2)]

D4, B3, A4, E5, D5, G5 = 62, 59, 69, 76, 74, 79

SOUNDS = {
    "finished": note(D5, 0.30, BELL, 0.10),
    "question": sequence(note(A4, 0.12, PLUCK, 0.06), note(E5, 0.34, PLUCK, 0.12)),
    "hold": sequence(
        note(G5, 0.09, WOODBLOCK, 0.03), silence(0.09),
        note(G5, 0.09, WOODBLOCK, 0.03), silence(0.09),
        note(G5, 0.09, WOODBLOCK, 0.03),
    ),
    "failure": sequence(note(D4, 0.20, TRIANGLE, 0.12), silence(0.04), note(B3, 0.30, TRIANGLE, 0.15)),
    "quota": note(A4, 0.80, DETUNED_SOFT, 0.30, attack_seconds=0.08),
}


def level_match(samples, event_name):
    rms = np.sqrt(np.mean(samples**2))
    target_dbfs = TARGET_RMS_DBFS + GAIN_OFFSET_DB.get(event_name, 0.0)
    scaled = samples * (10 ** (target_dbfs / 20) / rms)
    ceiling = 10 ** (PEAK_CEILING_DBFS / 20)
    peak = np.max(np.abs(scaled))
    return scaled * min(1.0, ceiling / peak)


def write_wav(path, samples):
    pcm = (np.clip(samples, -1, 1) * 32767).astype("<i2")
    with wave.open(str(path), "wb") as wav_file:
        wav_file.setnchannels(1)
        wav_file.setsampwidth(2)
        wav_file.setframerate(SAMPLE_RATE)
        wav_file.writeframes(pcm.tobytes())


if __name__ == "__main__":
    output_dir = Path(sys.argv[1])
    output_dir.mkdir(parents=True, exist_ok=True)
    for event_name, samples in SOUNDS.items():
        write_wav(output_dir / f"{event_name}.wav", level_match(samples, event_name))
```

Measured result of that run (16-bit mono, 44.1 kHz):

| Event | Length | Peak (dBFS) | RMS (dBFS) |
| --- | --- | --- | --- |
| finished | 0.30 s | -11.8 | -25.0 |
| question | 0.46 s | -10.1 | -22.0 |
| hold | 0.45 s | -7.1 | -22.0 |
| failure | 0.54 s | -11.6 | -22.0 |
| quota | 0.80 s | -10.5 | -22.0 |

Findings from the run:

- **Match by RMS, not by LUFS.** ffmpeg's `ebur128` reports an integrated loudness of -70 LUFS
  (no measurement) for the 0.3 s file, because the EBU R128 measure needs longer sounds. So
  `loudnorm` does not suit these; the sketch scales each file to the same RMS and caps the
  peak.
- **Equal RMS leaves unequal peaks.** The `hold` file, partly silence between pulses, peaks
  about 3 to 4.7 dB above the others. Whether that sounds balanced is for the audition; the
  constants at the top are the knobs.
- **The sketch carries the "frequent is quieter" rule.** `GAIN_OFFSET_DB` lowers `finished`
  by 3 dB (-25 dBFS RMS against -22) on the assumption that it fires most often: the notifier
  raises it whenever a turn completes. The offset is a guess; set it from the owner's event
  log.
- **Only one sound plays per batch.** Sounds never overlap, so no combination of two has to be
  designed; what matters is that each of the five is told apart alone, and the most urgent
  event wins the batch.
- **One note needs no script.** `ffmpeg -f lavfi -i "sine=frequency=587.33:duration=0.3"
  -af "afade=t=in:d=0.005,afade=t=out:st=0.05:d=0.25:curve=exp,volume=-12dB" -ac 1 out.wav`
  made a 0.30 s file; it is a plain sine, so softer and less bell-like than the sketch.
- **`.oga` works.** `ffmpeg -i question.wav -c:a libvorbis -q:a 4 question.oga` produced a
  0.46 s Ogg file of about 5.6 KB (the WAV is about 40 KB). The five WAVs total about 220 KB.

Licence notes, only where they matter for what Torlek creates:

- A sound made by this recipe is Torlek's own work under the repository's Apache-2.0 licence;
  the recipe embeds no sample from any other project, so there is nothing to attribute.
- Copying another project's files, game audio or stock-site files into Torlek is out of scope
  by Theo's clarification. Referring to the freedesktop theme by name, as today, ships nothing
  (the package mixes CC-BY, CC-BY-SA and GPL licences, per its Arch Linux metadata).
- Audio generated by an AI model (Google's "Ethereal Vibration" ringtone began as such a clip)
  has unsettled licence questions; this guide does not use it.
- A recording Theo makes himself is his own work.

## Options

Costs are rough guesses for one agent with focused tests, as in the other reports.

### For Torlek's defaults

**Option A: ship no sounds; keep the freedesktop fallback (what #1130 asks for).** The owner
points each event at a file. Pros: nothing to maintain, matches #1130's acceptance. Cons:
users who dislike the theme still have to make their own sounds.

**Option B: keep the recipe in the repository and commit the generated files.** A generator
plus a test that regenerates and compares keeps the files honest, if the comparison has a
tolerance (or a pinned numpy): sine and exponential results can differ in the last bit between
numpy builds, and 16-bit rounding turns that into different samples. Pros: greppable, no new
runtime code, the files are Torlek's. Cons: about 220 KB of binaries in git; about half a day
including auditions with Theo.

**Option C: synthesize at first use in Rust.** About 100 lines of sine math, no files in git.
Pros: no binaries, parameters in code. Cons: product code and tests in the notifier, so it
belongs to a follow-up after #1127 and #1130; about a day.

### For Theo's own set (kept outside the repository)

**Option 1: run the sketch and tune the numbers.** Change the notes, lengths and the two level
constants, play the five files, repeat. About 30 minutes for the first pass.

**Option 2: ffmpeg one-liners per event.** Simpler and plainer (sines, fades); about 20
minutes. Good for a first listen, less varied in timbre.

**Option 3: record his own sounds.** A finger on a glass, a voice, an instrument, trimmed and
level-matched with ffmpeg. Fully his; the time depends on how particular he is.

## Recommendation (proposal)

1. **#1130 as written (Option A).** No audio goes into the repository; the fallback stays the
   freedesktop name.
2. **For Theo's own sounds: Option 1 now.** Run the sketch, audition the five files starting
   with finished against quota, adjust, and set the five paths once #1130 lands. Use `.wav`,
   which #1130 names as safe.
3. **For Torlek's defaults: decide after Theo has lived with his set.** If he likes the result,
   file a follow-up for Option B (committed files and a regenerating test), after #1130 so it
   uses the real settings and `torlek notify test`. Do not start it from this report.

Additions to #1130's text (proposed, Theo applies them):

> - The user guide names `.oga` and `.wav` as safe formats and says an mp3 depends on the
>   player. (Herdr accepts mp3 only; kitty accepts WAV and OGA on Linux.)
> - The user guide notes that an agent's own terminal bell (Claude Code `terminal_bell`, Codex
>   `bel`) is a second sound path the notifier does not control. The notifier itself has no
>   bell code.
> - The user guide says no sound ships with Torlek and links the recipe in this report.

## Decisions for the owner

Proposals only; none is decided.

1. Keep Option A for #1130, or also ship defaults (B or C)? Recommended: A now.
2. Does Theo want to start from the sketch (Option 1), one-liners (Option 2) or recordings
   (Option 3)? Recommended: 1.

## Sources, limits and what was not checked

All pages were read on 2026-11-07. A date in the second column is the source's own date where
it gave one.

| Source | Date | Used for |
| --- | --- | --- |
| [Herdr config reference](https://herdr.dev/tui/docs/config-reference/) and [repository](https://github.com/herdrdev/herdr) (`src/config/sound.rs`, `src/sound.rs`, `assets/sounds/`, read through the GitHub API) | Sounds added 2026-03-27 (`0403775f`) | Keys, formats, embedded files |
| [Herdr discussion #2529](https://github.com/herdrdev/herdr/discussions/2529) | Undated | The terminal bell is a separate path |
| [ogadra/dotfiles PR #448](https://github.com/ogadra/dotfiles/pull/448) | Opened 2026-11-02, merged 2026-11-03 | Five level changes on two files |
| [OpenCode repository](https://github.com/anomalyco/opencode) (`packages/ui/src/assets/audio`, `packages/tui/src/attention-sounds.node.ts`) and [PR #26980](https://github.com/anomalyco/opencode/pull/26980) | Merged 2026-05-13 | Families, per-event defaults |
| [Zed repository](https://github.com/zed-industries/zed) (`assets/sounds/`, `docs/src/reference/all-settings.md`), commit `d211f88d` | 2025-05-27 | One shared agent sound |
| [Claude Code terminal configuration](https://code.claude.com/docs/en/terminal-config) and [hooks reference](https://code.claude.com/docs/en/hooks) | Undated | Bell and hook options |
| [Codex advanced configuration](https://developers.openai.com/codex/config-advanced) (redirects to learn.chatgpt.com) | Undated | `notify`, `notification_method` |
| [Warp agent notifications](https://docs.warp.dev/agents/capabilities/agent-notifications/) | Undated; found through search | No sound setting found |
| [Ghostty `Config.zig`](https://github.com/ghostty-org/ghostty/blob/main/src/config/Config.zig) and [kitty `definition.py`](https://github.com/kovidgoyal/kitty/blob/master/kitty/options/definition.py) | `main` and `master` as read | Bell options |
| [alexop.dev post](https://alexop.dev/posts/how-i-added-sound-effects-to-claude-code-with-hooks/), [`opencode-plugin-boops`](https://www.pkgstats.com/pkg:opencode-plugin-boops), [`opencode-starcraft`](https://github.com/spartandingo/opencode-starcraft) | 2026-02-11; v2.7.0 without exact date; undated | Community sound sourcing |
| [Brewster, Wright and Edwards, Guidelines for the Creation of Earcons](https://www.dcs.gla.ac.uk/~stephen/earcon_guidelines.shtml) | Year not on the page | Length, loudness, timbre, rhythm |
| [Zafrin and O'Sullivan, Good Vibrations](https://design.google/library/ux-sound-haptic-material-design) | Undated | How Google made its sounds; hierarchy, palette, chords |
| [Ahva, Futurice](https://www.futurice.com/blog/designing-notification-sounds) | 2017-09-05 | Pitch, accent, shared motif, tritone |
| [sfxr readme (DrPetter), via sfxr-qt](https://github.com/agateau/sfxr-qt/blob/478f69bd0de0e129fd29f5f2c0a7499f70621a2d/readme-sfxr-sdl.txt) and [pyfxr documentation](https://pyfxr.readthedocs.io/en/stable/generating.html) | 2007-12-14; undated | Envelope vocabulary, waveforms, MIT licence of sfxr |
| [Kaygusuz and Zuluaga, Impact of Intervals on the Emotional Effect in Western Music](https://arxiv.org/abs/1812.04723) | 2018-12-10 | Intervals relative to a key |
| [musicscience.net summary](https://musicscience.net/2023/11/21/how-automatic-are-emotional-responses-to-musical-chords/) | 2023-11-21 | Caveat: automatic effects of chords are weaker than self-reports |
| [MS Indy on IEC 60601-1-8](https://mspindy.com/technical-articles/using-audible-alarms-in-medical-equipment-iec-60601-1-8/) | Undated | Pulse count and pitch range (secondary) |
| [Arch `sound-theme-freedesktop`](https://archlinux.org/packages/extra/any/sound-theme-freedesktop) | Package 0.8-6, built 2024-07-05 | Licence mix of the theme |
| Local checks: `ffmpeg -h filter=sine`, `aevalsrc`, `loudnorm` and the recipe run | 2026-11-07 | Options, measurements |
| `crates/cockpit-notify/README.md` and `src/outlets.rs` | At `1a7d869b` | Events, default sound names, one sound per batch |
| Issues [#1130](https://github.com/Androlax2/agent-cockpit/issues/1130), [#1127](https://github.com/Androlax2/agent-cockpit/issues/1127) | 2026-11-07 | Scope |

Limits:

- No sound was heard. The recipe's lengths, levels and the claim that two sounds are distinct
  are measured or reasoned, not auditioned; Theo must listen before choosing.
- How Herdr, OpenCode and Zed made their audio was not found; absence of a statement is not a
  statement that nothing exists.
- The interval research is about melodies in music, not notification sounds; it supports "end
  on a different scale degree", not any single interval.
- The Material sound pages and the Edworthy alarm-design paper could not be fetched; the first
  is represented by the Google Design feature, the second by a trade article.
- Which formats `pw-play`, `paplay` and `canberra-gtk-play` decode on Theo's machine was not
  tested (playing them would use the live audio service); the document for #1130 should state
  them after a test with a fake player and one real run.
- Warp and iTerm2 behaviour rests on documentation pages or search summaries, not on running
  the products. Licence summaries are not legal advice.
