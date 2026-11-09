# Worker model trials database

For the coordinator closing out a worker, and for anyone regenerating the model card. The
method behind it is in [the benchmarking research](../2026-11-09-1264-model-benchmarking.md).

- `model-trials.csv`: one row per merged worker pull request.
- `model_card.py`: prints the card (one row per model, effort and task kind) from the CSV.

## Add a row at close-out

Append one line to `model-trials.csv` when the worker's pull request merges, in the same
change that records anything else about the worker. A worker that opened several pull
requests gets one row per pull request with the same `issue`.

| Column | What to write |
| --- | --- |
| `merged` | Merge date, `YYYY-MM-DD` (UTC) |
| `issue` | `#N`; several issues as `#N+#M` |
| `pr` | Pull request number, no `#` |
| `harness` | `claude` or `codex` |
| `model` | `Opus`, `Sonnet`, `Fable`, `Haiku`, or the Codex model name (`Sol`, `Astra`, `Terra`, `Luna`); `?` when unknown |
| `effort` | The effort the session started with; `?` when unknown |
| `kind` | `security`, `risky` (cross-crate, wire, state), `contained`, `docs` or `research` |
| `trial` | `default` (the rule's choice), `trial` (a deliberate deviation), `control` (a default pull request that also got the independent review) or `replay` (a re-run of a past issue) |
| `lines_added` | Added lines of the merged diff (`gh pr view N --json additions`) |
| `ci_fix_rounds` | Pushes made to fix a failed check, as the watcher saw them; reruns overwrite the run history |
| `ci_cause` | Empty when `ci_fix_rounds` is 0. Otherwise one or more of `own` (a defect in the change), `check` (a lint, spelling, doc or CodeQL check the worker could have run first), `flake`, `drift` (main moved under the branch), `platform` (only the CI machine differs) or `unknown`, joined with `+` |
| `nudges` | Messages the coordinator sent to restart a stalled worker |
| `idles` | Times the worker stopped with work pending, nudged or not |
| `reviewer_model` | Model of the independent reviewer; empty when none ran |
| `reviewer_blocking` | Blocking findings in that review; filled with `reviewer_model` |
| `reviewer_minor` | Minor findings in that review |
| `open_to_merge_h` | Hours from the pull request opening to its merge |
| `regressions` | Defects later traced to this pull request; update the row when one is found |
| `notes` | One line in your own words. No quotes from a session, no names of people or private projects |

## Regenerate the card

```sh
python3 -I docs/research/1264/model_card.py
```

The script refuses a row whose columns or values do not match the table above, and names it.
It counts every row; a decision uses only `trial`, `control` and `replay` rows, one issue
counted once, by the [stopping rule](../2026-11-09-1264-model-benchmarking.md#stopping-rule).
A cell under 5 rows is `none`, under 20 `screening`, from 20 `decidable`. Read the
[stress test](../2026-11-09-1264-model-benchmarking.md#stress-test) before drawing a
conclusion from any cell.
