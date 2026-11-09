# Worker trials log

Frozen at 34 rows. New rows go to the trials database,
[`docs/research/1264/`](../../../docs/research/1264/README.md).

One row per merged worker PR, added by the coordinator when it ends the worker. The rotation
rule and the model and effort rule are in [SKILL.md](SKILL.md#model-and-effort).

The owner asked (2026-11-04) to keep testing models and effort levels so the coordinator
gets a good sample and improves how it uses workers.

**Rotation:** each start picks model and effort by the rule in SKILL.md, but on purpose about
one start in three deviates by one step (Fable instead of Opus or Sonnet, medium instead of
high, or low on a trivial docs change), so every cell gets samples. Never deviate downward on
security or data-loss work. Record each merged PR here when its worker ends. Re-compare every
ten new rows, update the rule in SKILL.md and tell the owner the change in one line.

**Columns:** issue, PR, model, effort, kind (risky, contained, docs, research), lines added,
CI fix rounds (as the watcher saw them, not `gh run view`: reruns overwrite), review passes,
open-to-merge time, notes. The model of a PR shows in its commit co-author line.

| issue | PR | model | effort | kind | +lines | CI fixes | reviews | open-merge | notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| #710 | 745 | Opus | high | risky | 1328 | 0 | 1 | 0.2h | |
| #713 | 792 | Fable | high | risky | 1086 | 1 | 1 | 6.1h | reboot + rebase inside |
| #714 | 810 | Fable | high | contained | 732 | 0 | 1 | 0.3h | |
| #770 | 795 | Sonnet | high | contained | 216 | 0 | 1 | 0.2h | |
| #771 | 796 | Sonnet | medium | contained | 23 | 0 | 1 | 0.6h | |
| #777 | 794 | Sonnet | medium | docs | 22 | 0 | 1 | 0.2h | |
| #764 | 803 | Sonnet | high | contained | 182 | 0 | 1 | 0.2h | |
| #800 | 801 | Sonnet | medium | contained | 13 | 1 | 1 | 5.5h | reboot inside; failure was main drift |
| #804 | 806 | Sonnet | high | contained | ? | ? | 1 | ? | measured 16 -> 1 parallel builds |
| #769 | 808 | Sonnet | medium | contained | ? | ? | 1 | ? | |
| #767 | 811 | Opus | high | risky | ? | ? | 2 | ? | second review pass |
| #706 | 812 | Opus | high | contained | ? | ? | 1 | ? | asked a design question first |
| #406 p2 | 797 | Opus | high | risky | 2013 | 0 | 1 | 5.7h | reboot inside |
| #766 | 807 | Opus | high | risky | 1046 | 1 | 1 | 0.9h | the CI failure was flaky tests (#816); the worker went idle and missed it until nudged |
| #406 p5 | 818 | Sonnet | medium | contained | 522 | 1 | 1 | 0.4h | deviation trial; one rebase conflict; CI failure log came back empty, a rerun passed |
| #754 | 819 | Opus | high | risky | 1104 | 1 | 1 | 0.5h | opened the PR before its review (auto-merge not armed); 3 checks failed on the pre-review commit |
| #816 | 821 | Opus | medium | contained | 105 | 0 | 1 | 0.2h | deviation trial; found a real product race (a busy session counted as a delivery try), not just a test race |
| #406 p4 | 824 | Fable | high | contained | 1375 | 0 | 1 | 0.2h | stalled once mid-task before the PR; clean CI after |
| #546 | 827 | Opus | high | research | report | 0 | 1 | ? | asked D1-D6 one at a time; re-planned on the owner's direct reorder; filed as #828-#835 |
| #406 p3 | 820 | Opus | high | risky | 2553 | 1 | 1 | 1.3h | asked A/B/C on DB retention first; CI failure was the disk-floor flake (#838), rerun passed |
| #825 | 837 | Sonnet | high | contained | 31 | 1 | 1 | 0.5h | CI failure was the unrelated proxy port flake (#841); rerun passed; asked "log shows no test output" (gh run view hides it) |
| #838 | 839 | Sonnet | medium | contained | 50 | 0 | 1 | 0.25h | deviation trial; clean |
| #136 | 843 | Opus | high | contained | 716 | 1 | 1 | 0.5h | lint fix (MD018); its runner tests killed the installed tmux server twice (#845), breaking the isolation it was told to keep |
| #846 | 849 | Fable | medium | contained | 88 | 0 | 1 | 0.2h | deviation trial; clean; one stray-looking idle at review, no nudge needed |
| #850 | 853 | Opus | high | risky | 1413 | 0 | 1 | 0.4h | asked a real spec gap (a Host header) before coding it; one stray needs-input "x" |
| #845 | 854 | Opus | high | risky | 565 | 1 | 1 | 0.4h | clippy useless format; proved the -L fallback in a mount namespace; product now always -S; workspace rule test added |
| #856 | 866 | Fable | high | research | report | 0 | 1 | ? | deviation trial; asked Q1-Q6 one at a time; review fixes pushed before auto-merge; changes posted on 8 issues |
| #857 | 867 | Sonnet | high | research | report | 0 | 1 | ? | asked D1-D4 one at a time, answered "why C over B" directly; changes posted on 8 issues |
| #859 | 868 | Sonnet | high | research | report | 0 | 1 | ? | asked Q1-Q3 one at a time; measured cache sizes; changes posted on 10 issues |
| #855 | 870 | Opus | high | risky | 2487 | 0 | 1 | 1.5h? | phone layout + read-only phone TUI; review fixes then gate re-run; clean CI |
| #861 | 876 | Sonnet | high | research | report | 0 | 1 | 0.6h | asked Q1-Q3 one at a time; changes posted on 8 issues; clean CI |
| #860 | 877 | Sonnet | high | research | report | 1 | 1 | 0.9h | asked Q1-Q4 one at a time; MD018 lint failure (line starting '#71'), fixed after my pointer; new issue #880 |
| #862 | 881 | Sonnet | high | research | report | 0 | 1 | 1.4h | asked Q1-Q4 one at a time; survived the alpha 3 daemon restart; changes posted on 9 issues |
| #858 | 887 | Opus | high | research | report | 0 | 1 | 2h | revised its options after #859 and re-asked Q1 rather than take a relayed old-list answer; changes posted on 7 issues |

Stalls (idle mid-task without a question, needing a nudge): #777 Sonnet, #766 Opus, #406 p4 Fable.
