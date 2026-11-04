# Measurement appendix for the compaction research

Researcher: Claude Code (Sonnet 5.5), 2026-11-04, for
[the main report](../2026-11-04-686-compaction-research.md) (issue #686). Everything here was read
from the harnesses' local logs by the scripts below, which print counts and token numbers only.
No message text, path or identifier was copied into the repository.

## Method and limits

| Harness | Source read | Context size used | Compaction marker |
| --- | --- | --- | --- |
| Claude Code 2.1.x | `~/.claude/projects/*/*.jsonl`, top-level transcripts, subagent (sidechain) records skipped | assistant `message.usage`: `input_tokens` + `cache_read_input_tokens` + `cache_creation_input_tokens` | `system` record, subtype `compact_boundary`: `trigger`, `preTokens`, `postTokens`, `durationMs`; the next `isCompactSummary` user record is the summary |
| Codex 0.16x | `~/.codex/sessions/**/*.jsonl` | `event_msg` `token_count`: `last_token_usage.input_tokens`; window `model_context_window` | `compacted` record; a turn is open between `task_started` and `task_complete` or `turn_aborted` |
| OpenCode 2 | `~/.local/share/opencode/opencode.db`, opened read-only | assistant message `tokens`: input + cache read + cache write | assistant message with `mode` `compaction` and `summary` set; `part` records of type `compaction` carry `auto` |

- The logs hold all of Theo's sessions on this machine, not only Torlek-launched ones, over the
  harnesses' retention. Counts are per log file or database session.
- Tokens are the providers' own counts. Bytes (the brief) and tokens are never converted.
- "Mid-turn" for Claude Code means the record before the compaction boundary is a tool result;
  for Codex that a turn was open; for OpenCode that the last step before the compaction ended in
  tool calls. A user prompt or assistant text before the boundary is counted as not mid-turn.
- The threshold arithmetic in the report (compactions per million new tokens, mean carried
  context) is a model: a context that grows linearly from the median post-compaction size
  (23,678 tokens) to the threshold of a 300,000-token window, with the measured medians for time
  (76 s) and the first call's cache write (48,864 tokens). It is not an observation.
- Reference retention counts a reference as kept if its text appears again in the summary
  (case-insensitive substring). References are `#` and two to five digits, branch-like words
  (`word/123-slug`), commit-like hexadecimal words of seven to twelve characters containing a
  digit and a letter, and base names of files an `Edit`, `Write`, `MultiEdit` or `NotebookEdit`
  call touched. The regular expressions are in the script and are approximate.
- Reading was by script with `python3 -I`; the OpenCode database was opened with `mode=ro`. One
  exploratory query printed some stored message text to the terminal of the session that
  produced this report; nothing from it was kept.

## Results not shown in the report

- Claude Code peak context by model (sessions, p50 / p90 / max): Opus 5.5 275, 175,179 / 273,299
  / 966,801; Fable 5.1 23, 182,337 / 912,256 / 967,920; Haiku 5.5 24, 44,937 / 56,532 / 60,853;
  Sonnet 5.5 7, 92,530 / 156,719 / 233,936.
- Claude Code sessions with a peak of at least 100,000 / 200,000 / 250,000 / 400,000 / 800,000
  tokens: 227 / 130 / 101 / 24 / 13 of 332.
- Claude Code assistant calls between two compactions on the 300,000-token window: p50 190, p90
  293. Eight manual compactions (5 followed by a model call): p50 65.1 s, post-compaction p50
  15,731 tokens.
- Codex sessions reaching 50 / 60 / 70 / 80 / 90 % of their window: 29 / 26 / 22 / 19 / 16 of 81.
- OpenCode sessions with a peak of at least 100,000 / 150,000 / 200,000 / 300,000 tokens: 125 /
  67 / 37 / 18 of 566. The wall time of compaction has outliers of weeks (interrupted and
  resumed sessions); the median is used.
- Installed binaries were searched for option names only (`grep -a -c`); no option was run.

## Scripts

Run each with `python3 -I`. They are reproduced as run.

### Claude Code: how full, when it compacts, cost

```python
"""Numeric-only measurement of Claude Code transcripts. Never prints message content or paths."""
import json, glob, os, statistics as st, collections, datetime as dt, sys

def pct(values, p):
    values = sorted(values)
    if not values: return None
    k = (len(values) - 1) * p / 100
    lo, hi = int(k), min(int(k) + 1, len(values) - 1)
    return round(values[lo] + (values[hi] - values[lo]) * (k - lo))

sessions = []
events = []
for path in glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl')):
    peak = 0; turns = 0; models = collections.Counter(); first = last = None
    series = []  # (ctx, ts)
    prev = None  # previous main-chain record summary
    pending_boundary = None
    compactions = 0
    for line in open(path, errors='replace'):
        try: d = json.loads(line)
        except Exception: continue
        if d.get('isSidechain'): continue
        t = d.get('type'); ts = d.get('timestamp')
        if ts:
            first = first or ts; last = ts
        if t == 'system' and d.get('subtype') == 'compact_boundary':
            m = d.get('compactMetadata', {})
            pending_boundary = dict(trigger=m.get('trigger'), pre=m.get('preTokens'), post=m.get('postTokens'),
                                    dur=m.get('durationMs'), prev=prev, ts=ts, model=None, session=path)
            compactions += 1
            continue
        if t == 'assistant':
            msg = d.get('message', {}); u = msg.get('usage')
            if not u or msg.get('model') == '<synthetic>': continue
            ctx = u.get('input_tokens', 0) + u.get('cache_read_input_tokens', 0) + u.get('cache_creation_input_tokens', 0)
            if ctx == 0: continue
            peak = max(peak, ctx); turns += 1; models[msg.get('model')] += 1
            series.append((ctx, ts))
            if pending_boundary is not None:
                pending_boundary['next_ctx'] = ctx
                pending_boundary['next_create'] = u.get('cache_creation_input_tokens', 0)
                pending_boundary['next_read'] = u.get('cache_read_input_tokens', 0)
                pending_boundary['model'] = msg.get('model')
                events.append(pending_boundary); pending_boundary = None
            prev = 'assistant_tool' if any(c.get('type') == 'tool_use' for c in msg.get('content', []) if isinstance(c, dict)) else 'assistant_text'
        elif t == 'user':
            c = d.get('message', {}).get('content')
            if isinstance(c, list) and any(isinstance(x, dict) and x.get('type') == 'tool_result' for x in c): prev = 'tool_result'
            elif d.get('isCompactSummary'): pass
            else: prev = 'user_prompt'
    if turns:
        sessions.append(dict(peak=peak, turns=turns, compactions=compactions, model=models.most_common(1)[0][0], first=first, last=last))

print('sessions with usage:', len(sessions))
peaks = [s['peak'] for s in sessions]
print('peak context tokens p50/p75/p90/p99/max:', [pct(peaks, p) for p in (50, 75, 90, 99, 100)])
for name, pred in (('>=100k', 100_000), ('>=200k', 200_000), ('>=250k', 250_000), ('>=400k', 400_000), ('>=800k', 800_000)):
    print('sessions peak', name, sum(1 for p in peaks if p >= pred))
print('sessions with >=1 compaction:', sum(1 for s in sessions if s['compactions']), 'total boundaries:', len(events))
by_model = collections.defaultdict(list)
for s in sessions: by_model[s['model']].append(s['peak'])
for m, v in by_model.items(): print('model', m, 'sessions', len(v), 'peak p50/p90/max', pct(v, 50), pct(v, 90), max(v))
print('trigger counts:', collections.Counter(e['trigger'] for e in events))
for trig in ('auto', 'manual'):
    ev = [e for e in events if e['trigger'] == trig]
    if not ev: continue
    pre = [e['pre'] for e in ev if e['pre']]; post = [e['post'] for e in ev if e['post'] is not None]; dur = [e['dur'] for e in ev if e['dur']]
    print(trig, 'n', len(ev), 'pre p10/p50/p90/min/max', [pct(pre, p) for p in (10, 50, 90, 0, 100)], 'post p50/p90', pct(post, 50), pct(post, 90),
          'dur s p50/p90/max', round(pct(dur, 50) / 1000, 1), round(pct(dur, 90) / 1000, 1), round(max(dur) / 1000, 1))
    print('  total compaction wall time (h):', round(sum(dur) / 3.6e6, 2), 'kept ratio post/pre p50:', round(st.median(e['post'] / e['pre'] for e in ev if e['pre'] and e['post'] is not None), 3))
    print('  by model:', collections.Counter(e['model'] for e in ev))
    print('  position before boundary:', collections.Counter(e['prev'] for e in ev))
    nc = [e.get('next_create') for e in ev if e.get('next_create') is not None]; nr = [e.get('next_read') for e in ev if e.get('next_read') is not None]
    print('  first call after: cache_create p50/p90', pct(nc, 50), pct(nc, 90), 'cache_read p50', pct(nr, 50), 'n', len(nc))
    pre_models = collections.Counter()
auto_pre = sorted(e['pre'] for e in events if e['trigger'] == 'auto' and e['pre'])
print('auto pre values (all):', auto_pre)
print('boundaries by day:', sorted(collections.Counter(e['ts'][:10] for e in events).items()))
```

### Claude Code: is there a turn end before the harness compacts

```python
"""For each auto-compacted Claude Code segment with a 300k window: was there a turn end (assistant
end_turn) after the context crossed a threshold and before the harness compacted? Numbers only."""
import json, glob, os, collections, datetime as dt

def pct(v, p):
    v = sorted(v)
    if not v: return None
    k = (len(v) - 1) * p / 100; lo = int(k); hi = min(lo + 1, len(v) - 1)
    return round(v[lo] + (v[hi] - v[lo]) * (k - lo))

def ts(x): return dt.datetime.fromisoformat(x.replace('Z', '+00:00')).timestamp()

WINDOW = 300_000
segments = []
for path in glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl')):
    seg = dict(points=[], calls=0)
    for line in open(path, errors='replace'):
        try: d = json.loads(line)
        except Exception: continue
        if d.get('isSidechain'): continue
        t = d.get('type')
        if t == 'system' and d.get('subtype') == 'compact_boundary':
            m = d['compactMetadata']
            if seg['calls']:
                seg.update(pre=m['preTokens'], trigger=m['trigger'], t_end=ts(d['timestamp']))
                segments.append(seg)
            seg = dict(points=[], calls=0)
        elif t == 'assistant':
            msg = d.get('message', {}); u = msg.get('usage')
            if not u or msg.get('model') == '<synthetic>': continue
            ctx = u.get('input_tokens', 0) + u.get('cache_read_input_tokens', 0) + u.get('cache_creation_input_tokens', 0)
            if not ctx: continue
            seg['calls'] += 1
            seg['points'].append((ctx, ts(d['timestamp']), msg.get('stop_reason')))
auto = [s for s in segments if s['trigger'] == 'auto' and s['pre'] < 500_000]
print('auto segments on the 300k window:', len(auto))
print('assistant calls per segment p50/p90:', pct([s['calls'] for s in auto], 50), pct([s['calls'] for s in auto], 90))
for frac in (0.5, 0.6, 0.7, 0.8):
    thr = frac * WINDOW; have = 0; lag_tokens = []; lag_secs = []; remaining_calls = []
    for s in auto:
        crossed = next((i for i, (c, _, _) in enumerate(s['points']) if c >= thr), None)
        if crossed is None: continue
        tail = s['points'][crossed:]
        remaining_calls.append(len(tail))
        stop = next((p for p in tail if p[2] == 'end_turn'), None)
        if stop:
            have += 1; lag_tokens.append(stop[0] - thr); lag_secs.append(stop[1] - tail[0][1])
    n = len(remaining_calls)
    print(f'{int(frac*100)}% ({int(thr)}): segments crossing {n}; with an end_turn before harness compaction {have}; '
          f'calls between crossing and compaction p50/p90 {pct(remaining_calls, 50)}/{pct(remaining_calls, 90)}; '
          f'first idle point: tokens past threshold p50/p90 {pct(lag_tokens, 50)}/{pct(lag_tokens, 90)}, seconds after crossing p50/p90 {pct(lag_secs, 50)}/{pct(lag_secs, 90)}')
```

### Claude Code: idle gaps at turn ends

```python
"""Idle gaps at a real turn boundary (assistant end_turn followed by a user prompt) by context size. Numbers only."""
import json, glob, os, datetime as dt, collections
def ts(x): return dt.datetime.fromisoformat(x.replace('Z', '+00:00')).timestamp()
buckets = collections.Counter(); total = collections.Counter()
for path in glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl')):
    last_assist = None
    for line in open(path, errors='replace'):
        try: d = json.loads(line)
        except Exception: continue
        if d.get('isSidechain') or not d.get('timestamp'): continue
        t = d.get('type')
        if t == 'assistant':
            msg = d.get('message', {}); u = msg.get('usage')
            if not u or msg.get('model') == '<synthetic>': continue
            ctx = u.get('input_tokens', 0) + u.get('cache_read_input_tokens', 0) + u.get('cache_creation_input_tokens', 0)
            last_assist = (ts(d['timestamp']), ctx, msg.get('stop_reason')) if ctx else last_assist
        elif t == 'user' and last_assist:
            c = d.get('message', {}).get('content')
            if isinstance(c, list) and any(isinstance(x, dict) and x.get('type') == 'tool_result' for x in c): continue
            if d.get('isCompactSummary') or d.get('isMeta'): continue
            gap = ts(d['timestamp']) - last_assist[0]
            if last_assist[2] == 'end_turn' and last_assist[1] >= 150_000:
                total['all'] += 1
                for name, lo in (('>=5min', 300), ('>=1h', 3600), ('>=8h', 28800)):
                    if gap >= lo: buckets[name] += 1
            last_assist = None
print('turn boundaries with context >=150k tokens:', total['all'])
print('  followed by an idle gap', dict(buckets))
```

### Claude Code: what the summary mentions again

```python
"""What of the conversation's concrete references does Claude Code's summary mention again?
Counts only; no text, path or identifier is printed."""
import json, glob, os, re, collections

ISSUE = re.compile(r'(?<![\w/])#(\d{2,5})\b')
BRANCH = re.compile(r'\b[a-z]+/\d+-[a-z0-9-]{3,}')
SHA = re.compile(r'\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,12}\b')

def text_of(content):
    if isinstance(content, str): return content
    out = []
    for block in content or []:
        if isinstance(block, dict) and block.get('type') == 'text': out.append(block.get('text', ''))
    return '\n'.join(out)

found = collections.Counter(); kept = collections.Counter(); per_event = []
sizes = []
for path in glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl')):
    items = {k: {} for k in ('issue', 'branch', 'file', 'sha')}  # value -> index of last mention
    position = 0; awaiting_summary = False
    for line in open(path, errors='replace'):
        try: d = json.loads(line)
        except Exception: continue
        if d.get('isSidechain'): continue
        t = d.get('type')
        if t == 'system' and d.get('subtype') == 'compact_boundary':
            awaiting_summary = True; continue
        if awaiting_summary and t == 'user' and d.get('isCompactSummary'):
            summary = text_of(d.get('message', {}).get('content'))
            sizes.append(len(summary))
            counts = {}
            for kind, mentions in items.items():
                if not mentions: continue
                seen = len(mentions)
                survived = sum(1 for value in mentions if value.lower() in summary.lower())
                found[kind] += seen; kept[kind] += survived
                # recency: split mentions by last-mention position
                ordered = sorted(mentions.items(), key=lambda kv: kv[1])
                half = len(ordered) // 2
                for label, subset in (('older', ordered[:half]), ('newer', ordered[half:])):
                    found[(kind, label)] += len(subset)
                    kept[(kind, label)] += sum(1 for value, _ in subset if value.lower() in summary.lower())
            items = {k: {} for k in ('issue', 'branch', 'file', 'sha')}
            awaiting_summary = False
            continue
        position += 1
        if t in ('user', 'assistant'):
            msg = d.get('message', {}); content = msg.get('content')
            visible = text_of(content) if t == 'assistant' or not (isinstance(content, list) and any(isinstance(x, dict) and x.get('type') == 'tool_result' for x in content)) else ''
            for m in ISSUE.findall(visible): items['issue'][m] = position
            for m in BRANCH.findall(visible): items['branch'][m] = position
            for m in SHA.findall(visible): items['sha'][m] = position
            if t == 'assistant' and isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get('type') == 'tool_use' and block.get('name') in ('Edit', 'Write', 'MultiEdit', 'NotebookEdit'):
                        fp = (block.get('input') or {}).get('file_path')
                        if isinstance(fp, str) and len(os.path.basename(fp)) >= 4: items['file'][os.path.basename(fp)] = position
print('summaries read:', len(sizes), 'summary chars p50/p90/max:', sorted(sizes)[len(sizes)//2], sorted(sizes)[int(len(sizes)*.9)], max(sizes))
for kind in ('issue', 'branch', 'file', 'sha'):
    if found[kind]:
        line = f'{kind}: distinct references before compaction {found[kind]}, mentioned again in the summary {kept[kind]} ({kept[kind]/found[kind]:.0%})'
        for label in ('older', 'newer'):
            if found[(kind, label)]: line += f'; {label} half {kept[(kind,label)]}/{found[(kind,label)]} ({kept[(kind,label)]/found[(kind,label)]:.0%})'
        print(line)
```

### Codex

```python
"""Numeric-only measurement of Codex rollouts. Never prints content or paths."""
import json, glob, os, collections, datetime as dt

def pct(v, p):
    v = sorted(v)
    if not v: return None
    k = (len(v) - 1) * p / 100; lo = int(k); hi = min(lo + 1, len(v) - 1)
    return round(v[lo] + (v[hi] - v[lo]) * (k - lo))

def ts(x):
    return dt.datetime.fromisoformat(x.replace('Z', '+00:00')).timestamp() if x else None

sessions = []; events = []
for path in glob.glob(os.path.expanduser('~/.codex/sessions/**/*.jsonl'), recursive=True):
    ctx = None; window = None; in_turn = False; peak = 0; n_tc = 0; comp = 0; pending = None; last_ts = None; models = set()
    prev_kind = None
    for line in open(path, errors='replace'):
        try: d = json.loads(line)
        except Exception: continue
        t = d.get('type'); p = d.get('payload') if isinstance(d.get('payload'), dict) else {}
        now = ts(d.get('timestamp'))
        if t == 'event_msg' and p.get('type') == 'task_started': in_turn = True
        elif t == 'event_msg' and p.get('type') in ('task_complete', 'turn_aborted'): in_turn = False
        elif t == 'event_msg' and p.get('type') == 'token_count' and p.get('info'):
            info = p['info']; last = info.get('last_token_usage') or {}
            ctx = last.get('input_tokens'); window = info.get('model_context_window') or window
            if ctx:
                peak = max(peak, ctx); n_tc += 1
                if pending is not None:
                    pending['post'] = ctx; pending = None
        elif t == 'compacted':
            comp += 1
            pending = dict(pre=ctx, window=window, in_turn=in_turn, prev=prev_kind, gap=(now - last_ts) if now and last_ts else None)
            events.append(pending)
        if t in ('response_item', 'event_msg'):
            prev_kind = p.get('type')
        last_ts = now or last_ts
    if n_tc: sessions.append(dict(peak=peak, window=window, comp=comp, turns=n_tc))

print('sessions with token_count:', len(sessions))
print('windows:', collections.Counter(s['window'] for s in sessions))
peaks = [s['peak'] for s in sessions]
print('peak ctx p50/p75/p90/max:', [pct(peaks, q) for q in (50, 75, 90, 100)])
rel = [s['peak'] / s['window'] for s in sessions if s['window']]
print('peak fraction of window p50/p90/max:', [round(sorted(rel)[int((len(rel)-1)*q/100)], 3) for q in (50, 90, 100)])
for th in (0.5, 0.6, 0.7, 0.8, 0.9):
    print(f'sessions reaching {int(th*100)}% of window:', sum(1 for r in rel if r >= th))
print('sessions with compaction:', sum(1 for s in sessions if s['comp']), 'events:', len(events))
pre = [e['pre'] for e in events if e['pre']]
frac = [e['pre'] / e['window'] for e in events if e['pre'] and e['window']]
print('pre ctx p10/p50/p90/min/max:', [pct(pre, q) for q in (10, 50, 90, 0, 100)])
print('pre as fraction of window p10/p50/p90/min/max:', [round(sorted(frac)[int((len(frac)-1)*q/100)], 3) for q in (10, 50, 90, 0, 100)] if frac else None)
post = [e['post'] for e in events if e.get('post')]
print('post ctx p50/p90:', pct(post, 50), pct(post, 90), 'n', len(post))
print('in turn at compaction:', collections.Counter(e['in_turn'] for e in events))
print('record before compaction:', collections.Counter(e['prev'] for e in events))
```

### OpenCode

```python
"""Numeric-only measurement of OpenCode's database. Never prints content."""
import sqlite3, json, os, collections

def pct(v, p):
    v = sorted(v)
    if not v: return None
    k = (len(v) - 1) * p / 100; lo = int(k); hi = min(lo + 1, len(v) - 1)
    return round(v[lo] + (v[hi] - v[lo]) * (k - lo))

db = sqlite3.connect('file:' + os.path.expanduser('~/.local/share/opencode/opencode.db') + '?mode=ro', uri=True)
rows = db.execute("select session_id, time_created, time_updated, "
                  "json_extract(data,'$.role'), json_extract(data,'$.mode'), json_extract(data,'$.summary'), "
                  "json_extract(data,'$.tokens.input'), json_extract(data,'$.tokens.output'), "
                  "json_extract(data,'$.tokens.cache.read'), json_extract(data,'$.tokens.cache.write'), "
                  "json_extract(data,'$.finish'), json_extract(data,'$.modelID') "
                  "from message order by session_id, time_created, id").fetchall()
by_session = collections.defaultdict(list)
for r in rows: by_session[r[0]].append(r)
peaks = []; events = []
for sid, msgs in by_session.items():
    peak = 0; prev_ctx = None; prev_finish = None; compaction_seen = 0
    for (_, created, updated, role, mode, summary, tin, tout, cr, cw, finish, model) in msgs:
        if role != 'assistant' or tin is None: continue
        ctx = (tin or 0) + (cr or 0) + (cw or 0)
        if mode == 'compaction' and summary:
            events.append(dict(pre=prev_ctx, finish_before=prev_finish, dur=(updated - created) / 1000, out=tout, model=model))
            compaction_seen += 1
            continue
        if events and events[-1].get('post') is None and compaction_seen and events[-1].get('sid') is None:
            pass
        peak = max(peak, ctx); prev_ctx = ctx; prev_finish = finish
    if peak: peaks.append(peak)
print('sessions with usage:', len(peaks))
print('peak ctx p50/p75/p90/max:', [pct(peaks, q) for q in (50, 75, 90, 100)])
for th in (100_000, 150_000, 200_000, 300_000):
    print('sessions peak >=', th, sum(1 for p in peaks if p >= th))
print('compaction messages:', len(events))
pre = [e['pre'] for e in events if e['pre']]
print('pre ctx p10/p50/p90/min/max:', [pct(pre, q) for q in (10, 50, 90, 0, 100)])
dur = [e['dur'] for e in events if e['dur'] > 0]
print('compaction wall s p50/p90/max:', pct(dur, 50), pct(dur, 90), max(dur) if dur else None)
print('summary output tokens p50/p90:', pct([e['out'] for e in events if e['out']], 50), pct([e['out'] for e in events if e['out']], 90))
print('finish of last step before compaction:', collections.Counter(e['finish_before'] for e in events))
print('models at compaction:', collections.Counter(e['model'] for e in events))
```
