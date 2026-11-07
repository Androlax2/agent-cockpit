# Local models: hardware fit, candidates, runtimes and a benchmark set

Appendix to [the #497 report](../2026-11-07-497-worker-model-selection.md). **Research and
proposal, 2026-11-07.** Nothing here was installed, downloaded or run: no model was pulled and
no benchmark was executed. The only commands run were the read-only probe below. All external
pages were fetched 2026-11-07.

## The probe

### Commands (read-only, local, nothing is sent anywhere)

```sh
lscpu | grep -E 'Model name|^CPU\(s\)|Thread'
free -g
lspci | grep -iE 'vga|3d|display'
cat /sys/class/drm/card*/device/mem_info_vram_total   # AMD GPUs; bytes
nvidia-smi --query-gpu=name,memory.total --format=csv # NVIDIA GPUs; absent here
df -h --output=avail "$HOME"
```

Apple silicon would read `sysctl hw.memsize` and `system_profiler SPDisplaysDataType` instead;
unified memory is shared by the model and everything else, so only about 60 to 70% of it is a safe
budget (*Est.*, common practice; not measured here).

### Result on this session's host (*Observed*)

The host the Torlek sandbox runs on, assumed to be Theo's machine:

| Item | Value |
| --- | --- |
| CPU | AMD Ryzen 7 7800X3D, 8 cores, 16 threads |
| RAM | about 30 GiB total, 20 GiB available while other sessions ran |
| GPU | AMD Radeon, PCI name "Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]" |
| VRAM | 16,304 MiB (sysfs), which excludes the 9070 GRE as an exact match |
| Free disk | 550 GB |
| NVIDIA GPU | none (`nvidia-smi` not found) |

Not observable by the probe: memory bandwidth, the exact GPU SKU, whether the desktop is using
VRAM, and which GPU backend (ROCm or Vulkan) the runtime will pick.

## How the estimate is made

Four rules, each *Est.* and checkable by running the model once:

1. **Weights.** Memory for the weights is roughly parameters times bits per weight divided by 8,
   plus a few percent for tensors kept at higher precision. 4-bit is about 0.5 to 0.6 GB per
   billion parameters. The vendor figures below are used where a page states them.
2. **KV cache.** Context memory is `2 x layers x KV heads x head size x context length x bytes
   per value`. It depends on the architecture, so it needs the model's configuration; guides
   quote it as the first limit for long contexts (the Kilo guide notes a dense model needing
   about 40 GB of KV cache at 262K tokens).
3. **Fit.** A model fits the GPU when weights + KV cache + about 1.5 to 2 GiB (runtime and
   display) is at most the VRAM. Otherwise some layers or experts move to system RAM and
   generation slows sharply, as the October guide says.
4. **Speed.** Generation is limited by memory bandwidth: tokens per second is at most bandwidth
   divided by the bytes read per token (the active weights; for a mixture-of-experts model, only the
   active experts). Real runs reach roughly 40 to 60% of that bound. Prompt processing is
   compute-bound and faster per token.

**How sure:** the rules give the order of magnitude (a factor of about 2). They cannot say
whether a given tool-calling stack stays reliable at 4-bit; that is a quality question for the
benchmark, not for arithmetic.

### What the rules say for this machine (*Est.*)

Assumption to confirm: both 16 GB RX 9000-series parts have about 640 GB/s of memory bandwidth
(from the published specification, not measured here).

| Model class (source of size) | Weights | Fits 16 GiB GPU? | Speed bound | Note |
| --- | --- | --- | --- | --- |
| Gemma 4 12B, 4-bit (8 GB download, [Hashnode guide](https://hashnode.com/blog/best-local-llm-for-coding), 2026-10-28) | about 8 GB | Yes, room for a long context | about 80 tokens/s bound; 30 to 50 realistic | Comfortable class |
| gpt-oss-20b (14 GB download, same guide) | about 14 GB | Borderline: about 2 GiB left for KV cache and overhead, so a small context | about 45 tokens/s bound; 18 to 27 realistic | The guides say tool calls need 64K context; likely needs offload |
| Qwen3.8-27B, 4-bit (14 GB at 4-bit, vendor figure via a [secondary summary](https://www.qubrid.com/blog-news/qwen38-27b-benchmarks-official-and-independent-results)) | about 14 GB | Borderline, same as above | about 45 tokens/s bound if fully on GPU | Offload to RAM slows it much further |
| Gemma 4 26B A4B, mixture of experts (16 to 19 GB download) | about 16 to 19 GB | No; experts spill to RAM | Faster than dense models of its size because few experts are active; not computable here | Candidate to test with offload |
| gpt-oss-120b (65 GB), Qwen3-Coder-Next (48 to 64 GB class) | 65 GB and up | No: exceeds 16 GiB and about 30 GiB RAM | not applicable | Cannot run |

## What each candidate is good for (third party and vendor, weak)

No source found compares a model that fits this machine with the hosted Claude models on
coding-agent tasks under the same harness. What exists:

| Claim | Source | Type and limits |
| --- | --- | --- |
| Qwen3.8-27B: 61.7 on SWE-bench Pro and 73.0 on Terminal-Bench 2.1, against 53.5 and 63.4 for Qwen3.6-27B; Apache-2.0; scored in the Claude Code harness. Against Opus 4.6 at max effort: ahead on SWE-bench Pro (61.7 vs 53.4), behind on Terminal-Bench 2.1 (73.0 vs 78.2). | [Qubrid summary](https://www.qubrid.com/blog-news/qwen38-27b-benchmarks-official-and-independent-results) of Qwen's numbers (found by search 2026-11-07) | Secondary of vendor numbers, different harnesses and an older Claude model; not independent. |
| The same SWE-bench Pro suite shows the Claude 5.5 family higher (a third-party page lists Sonnet 5.5 at 81.3 and Opus 5.5 at 89.9, at max effort) | [Apidog](https://apidog.com/blog/claude-sonnet-5-5-benchmarks/) | Secondary, not checked against the primary; shown only to indicate direction. |
| Devstral Small 2: 68.0% on SWE-bench Verified; "aimed at structured tool-use workflows"; about 32K context on a 24 GB card. | [Kilo guide](https://blog.kilo.ai/p/the-best-local-coding-models-for) (2026-07-29) | Guest post by the maker of the inference engine it recommends; benchmark source not named. |
| Tool-call formatting "may degrade at Q4 for long agentic tasks"; try Q6. At least 64K context; short context is the first thing to check when tool calls fail. | Kilo guide; [Hashnode guide](https://hashnode.com/blog/best-local-llm-for-coding) (2026-10-28) | Practitioner guidance, not measured. |
| Insight's study ran Qwen3-Coder-Next, Kimi and GLM through a hosted router, not locally: Qwen3-Coder-Next needed 3 review cycles at about $12.69 and 69 minutes against 1 cycle for the strongest hosted arms. | [Insight Tech APAC](https://blog.apac.insight.com/2026/07/06/cost-to-a-merged-feature) (2026-07-06) | One task, one run; hosted, so it says nothing about local speed. |

Reading: open-weight models are close enough on vendor-chosen suites that a local option is worth
testing, but the claims are vendor-reported, tool-call reliability at 4-bit is unproven, and the
models that fit 16 GB are far smaller than the 27B class. **Proposed task kinds to test locally:**
documentation, mechanical single-file fixes with a focused test, and first-pass review. Not
architecture, security or data-loss work (the report's rule: never lower the model there).

## Runtimes and harnesses (support matrix)

Versions as released or installed on 2026-11-07.

| Runtime | Latest release seen | Notes |
| --- | --- | --- |
| Ollama | v0.40.15 (2026-11-03, [releases](https://github.com/ollama/ollama/releases)) | OpenAI-compatible endpoint on `127.0.0.1:11434`. Its default context depends on VRAM (4K under 24 GiB, per the Hashnode guide), so it must be raised to 64K or more for agent work. |
| llama.cpp | tag `v0.9.0` (2026-11-04, [releases](https://github.com/ggml-org/llama.cpp/releases)) | As the GitHub API reported it; the project's own build numbering was not checked. Direct control of quantization, KV cache type and offload. |
| LM Studio, vLLM | not checked | Named by OpenCode's documentation as discoverable runtimes. |

| Harness | Installed here | Local model support (docs) |
| --- | --- | --- |
| OpenCode | `opencode v2.2.5` (the repository's release tag read was v1.19.5, not reconciled) | [Documents](https://opencode.ai/docs/providers/) automatic discovery of Ollama (`127.0.0.1:11434`) and LM Studio (`127.0.0.1:1234/v1`); "discovered vLLM models start with tools disabled". Context-length handling for local runtimes is not documented on that page. |
| Codex CLI | `codex-cli 0.166.0` | [Documents](https://learn.chatgpt.com/docs/config-file/config-advanced) `--oss`, `oss_provider = "ollama"` or `"lmstudio"`, and custom `[model_providers.<id>]` with `wire_api = "responses"`. Project-local `.codex/config.toml` ignores `model_provider`; it must be set in `~/.codex/config.toml`. |
| Claude Code | `2.1.319` | [Documents](https://code.claude.com/docs/en/llm-gateway) gateways that expose a supported Anthropic API format, and states Anthropic "doesn't support routing Claude Code to non-Claude models through any gateway". A third-party guide (Hashnode, 2026-10-28) shows the base-URL variables for Ollama. Billing note from the same page: with a gateway credential set, traffic is billed per token to that credential's owner and the subscription's limits do not apply; setting only the base URL leaves the subscription active. |

## What Cockpit would need (*Proposed*)

1. **Capabilities.** Codex and OpenCode adapters report `Model` (and `Effort` where the harness
   has it); today only the Claude adapter does, so a start choosing a model there is refused
   (decision #701).
2. **A supervised model server.** Starting, health-checking and stopping the runtime as a managed
   process with a start-up time budget (model load can take minutes), owned by the daemon rather
   than by a worker.
3. **Resource admission.** GPU memory and RAM become admitted resources (#42), so two sessions
   cannot load models that together exceed VRAM and swap the desktop to a crawl.
4. **Sandbox forward.** The sandboxed harness reaches the runtime only through an explicit
   localhost forward (the host-service forward design, #860), never an open egress rule.
5. **Privacy as a constraint.** "Local only" must be checked against what the harness can reach,
   including its own telemetry and web tools, and refuse a start that could leave the machine.
6. **Accounting.** Each local run records wall time, tokens and, if the owner supplies a figure,
   energy. It is never recorded as zero cost.

## The local benchmark set (*Proposed*, not run)

Run by the owner, on the owner's machine, at no money cost. Five tasks, each a repository state
already used for a real merged change, so that hosted results can be compared later.

| Id | Kind | Source of the task | Executable check | Wall limit |
| --- | --- | --- | --- | --- |
| L1 | Bounded fix with a failing test | A merged small fix, parent commit | Focused `cargo nextest run --locked -p <crate>` | 30 minutes |
| L2 | Mechanical documentation edit | A merged docs change, parent commit | `markdownlint-cli2`, `typos`, offline `lychee` on the file | 20 minutes |
| L3 | Contained feature, under 150 lines | A merged contained change, parent commit | Focused tests from the change | 45 minutes |
| L4 | Review of a diff with a seeded defect | A merged change plus one planted, known bug | Blind check: did the review name the defect | 20 minutes |
| L5 | Short synthesis with sources | A closed research question | Rubric of required facts and a link check | 30 minutes |

Rules, so results are comparable and bounded:

- **Matched inputs.** Same commit, same brief text, same checks for every configuration; the brief
  is stored by hash.
- **Blind review.** Outcomes are judged by a reviewer who is not told which model produced which.
- **Repeats.** Three runs per task per configuration, since single runs are unreliable (Insight,
  METR). Report each outcome with its count, not a bare percentage.
- **Bounds.** A task stops at its wall limit; the whole set stops after a stated total (suggested
  4 hours) so power use and desk time are bounded. Run only while the machine is otherwise idle,
  so speed numbers are not distorted by other sessions.
- **No production side effects.** Work in a throwaway clone with no remote and no credentials.
- **Record with every result:** hardware (the probe table), runtime and version, model, exact
  quantization, context length, KV cache type, flags, harness and version, tokens, wall time,
  tokens per second, accepted yes or no, and interventions.
- **Hosted comparison.** Running the same five tasks on a hosted model uses allowance or money and
  is a separate approval (decision D4 in the report).

First results on Theo's machine: **none.** They need his agreement (decision D3) and a
confirmed GPU backend for this card, which the probe cannot establish.
