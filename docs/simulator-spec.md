# `rt-sim`: simulator specification and plan

Status: specification / design note. Nothing here is implemented yet.
Companion to [degradation-formulation.md](degradation-formulation.md) (referred to as
"the formulation" below). That document says *what* a degradation plan is. This one says
how we will run plans, and the baselines they are compared against, so the claims in
formulation §8 can be measured.

## 1. Purpose and scope

The simulator exists to answer three questions (formulation §8):

1. **Freshness.** Under overload, does a plan-driven runtime give lower sink data age and
   reaction time than FIFO, EDF, drop-oldest and drop-late?
2. **Safety.** Does it have zero violations of `safety` jobs, `all` edges and `(m, k)`
   at contract-preserving levels, while the naive baselines do violate them?
3. **Cost.** Is the per-event decision overhead O(1) and small?

So the simulator has to model, faithfully:

- periodic and event-driven job releases, with jitter;
- per-job execution times that can overrun the WCET;
- data flow along edges under the `latest` / `all` / `optional` semantics, including
  **which sensor samples each value was computed from**, which is what data age needs;
- a processor that runs jobs under a pluggable scheduling and shedding policy;
- an **independent oracle** that checks every contract, whatever the policy does.

**Non-goals:** cycle accuracy, caches, interrupts, OS or middleware overhead (beyond an
optional constant context-switch cost), real code execution, distributed or networked
nodes.

## 2. Design principles

| principle | consequence |
|---|---|
| **Policy and workload are separate** | The workload (sensor release times, per-job execution demand, injected overruns) is generated from a seed *before* any policy runs. Every policy sees the same trace (common random numbers), so comparisons are paired and differences are due to the policy alone. |
| **The oracle is not the policy** | The engine never stops a policy from dropping something. It records what happened, and a separate oracle checks contracts. Claim 2 is then measured, not assumed. A plan-driven policy that's buggy shows up as violations. |
| **Deterministic** | Integer time, fixed tie-breaking rules (§5.3), seeded RNG. Same input + seed ⇒ byte-identical event log. |
| **No MLIR dependency** | `rt-sim` depends on `rt-schema` (model + annotations) and `rt-passes` (the plan), not on `rt-dialect` / melior. It builds and tests without LLVM, and runs fast. |
| **Uniprocessor first** | The formulation's utilisation and ladder numbers are uniprocessor. The engine is written for `m` identical cores, but v1 validates and experiments on `m = 1`. |

## 3. Inputs: what the simulator expects

A run is defined by four inputs:

```
            ┌─────────────────────────┐
DAG + annotations (.dot) ─┬──► rt-passes ──► DegradationPlan ─┐
                          │                                    ▼
experiment config (.yaml) ┴──► workload generator ──► Trace ──► engine ◄── Policy
                                                                  │
                                                    event log ◄───┴───► oracle, metrics
```

### 3.1 DAG and annotations (`.dot`)

The existing `Dag` (nodes with `wcet`, sensors with `period`, edges) plus the
annotations from formulation §4. Annotations describe the *system*, so the compiler
needs them too. They live in the DOT file, not in the experiment config.

**Node attributes**

| attribute | applies to | values | default | used by the simulator for |
|---|---|---|---|---|
| `wcet` | all | ticks > 0 | required | budget, overrun detection, baseline priorities |
| `period` | sensor | ticks > 0 | required | release times |
| `offset` | sensor, periodic task | ticks ≥ 0 | 0 | phase of the first release |
| `criticality` | all | `safety`, `mission`, `best_effort` | `mission` before propagation | oracle (safety drops), priorities, per-class metrics |
| `mk` | all | `"m,k"` with 0 ≤ m ≤ k | none (no contract) | oracle, plan-driven skip checks |
| `activation` | task | see §4.4 | see §4.4 | when jobs are released |
| `deadline` | any task, required on the sink | ticks > 0 | see §4.6 | per-job deadlines, EDF, `(m, k)` completion |
| `batch_wcet` | tasks with an `all` input | ticks per extra queued value | 0 | execution demand of batched jobs (§4.5) |

**Edge attributes**

| attribute | values | default |
|---|---|---|
| `policy` | `latest`, `all`, `optional` | `latest` |
| `max_age` | ticks > 0 (only on `latest`) | none (unbounded) |

Defaults are chosen so that an un-annotated DAG (all existing examples) is still
simulatable: every edge is `latest`, nothing has a contract, and nothing is `safety`.

> **Prerequisite.** These annotations are roadmap step 1 of the formulation. The
> simulator needs the `rt-schema` model extended with them (`Node.criticality`,
> `Node.mk`, `Node.activation`, `Node.deadline`, edge `policy`/`max_age`) and the DOT
> frontend parsing them. The MLIR side is not needed for the simulator.

### 3.2 Degradation plan

Produced by `rt-passes` (roadmap step 2). The simulator only consumes it, either as the
Rust struct directly or loaded from the JSON export, so plans can be hand-written before
the passes exist. This is the **compiler → runtime contract**, so it is pinned down here:

```jsonc
{
  "dag": "pipeline",
  "nodes": {
    "detect": {
      "effective_criticality": "mission",
      "slack": 5000,
      "on_critical_path": true,
      "drop_legal": true,
      // nodes whose activation is lost if this one skips once (formulation §5)
      "drop_cone": [],
      // drop cone if this node skips twice in a row (max_age exhausted)
      "drop_cone_consecutive": ["fuse", "plan"],
      "skip_budget": { "m": 1, "k": 2 },   // effective (m,k) after composing over the cone
      "drop_gain": 0.242                    // utilisation freed per skipped activation stream
    }
    // ...
  },
  "ladder": [
    { "level": 0, "contract_preserving": true,  "actions": [] },
    { "level": 1, "contract_preserving": true,  "actions": [ { "skip": "segment", "every": 1 } ] },
    { "level": 2, "contract_preserving": true,  "actions": [ { "skip": "segment", "every": 1 },
                                                             { "skip": "detect",  "every": 2 } ] },
    { "level": 3, "contract_preserving": false, "mode": "safety_only" }
  ],
  // per-node static priority key: (criticality rank, slack), used when no level overrides
  "priority": { "actuate": [0, 5000], "imu": [0, 10180], "detect": [1, 5000] /* ... */ }
}
```

`{ "skip": n, "every": j }` means "skip one of every `j` activations of `n`"
(`every: 1` = skip all). `mode: safety_only` means release only nodes whose effective
criticality is `safety`. The ladder action vocabulary is deliberately small. Extending
it (e.g. "run `n` in a reduced mode") is a later change to both sides.

### 3.3 Experiment config (`.yaml`)

Everything that is about the *experiment* rather than the system: platform, execution
time models, fault injection, which policies to compare, run length and seeds.
`serde_yaml` is already a workspace dependency.

```yaml
dag: ../examples/pipeline.dot
plan: null                 # null = run rt-passes; or a path to a plan .json

platform:
  cores: 1
  preemptive: true
  context_switch: 0        # ticks charged per dispatch of a different job

run:
  horizon: 60_000_000      # ticks (60 s at µs)
  warmup: 2_000_000        # metrics ignore events before this
  seeds: [1, 2, 3, 4, 5]

exec:                      # execution-demand model, as a fraction of WCET
  default: { dist: uniform, lo: 0.6, hi: 1.0 }
  detect:  { dist: lognormal, median: 0.7, sigma: 0.25, cap: 3.0 }

sensors:                   # optional per-sensor release perturbation
  cam: { jitter: { dist: uniform, lo: 0, hi: 2000 } }

scenarios:
  - name: nominal
  - name: detect-overrun
    inject:
      - { node: detect, from: 10_000_000, to: 40_000_000, scale: 2.5 }
  - name: cam-burst
    inject:
      - { sensor: cam, from: 20_000_000, to: 25_000_000, period: 11_000 }

policies: [fifo, fp-rm, edf, drop-oldest, drop-late, plan]

output:
  dir: results/pipeline
  event_log: true          # JSONL, can be large
```

**Execution-time distributions** (all scale the node's WCET, then round to ticks ≥ 1):

| `dist` | parameters | purpose |
|---|---|---|
| `const` | `value` | exact, hand-checkable runs (default `value: 1.0` = always WCET) |
| `uniform` | `lo`, `hi` | typical-case variation |
| `lognormal` | `median`, `sigma`, `cap` | heavy-ish tail |
| `pareto` | `scale`, `shape`, `cap` | heavy tail, rare large overruns |
| `bimodal` | `p_slow`, `fast`, `slow` | mode-dependent cost (e.g. detector on busy scenes) |
| `trace` | `file` | replay measured execution times |

`cap` bounds the multiplier so a run can't stall forever on one sample.

**Injections** apply on top of the distribution inside a time window:
`scale` (multiply demand), `add` (add ticks), `period` (temporarily change a sensor's
period: bursts), `drop` (sensor sample lost with probability `p`).

## 4. Execution semantics

This section is the normative model. Anything a policy observes or a metric reports is
defined in these terms.

### 4.1 Time

Integer ticks (`i64`), µs by convention. The simulation runs from `t = 0` to
`horizon`. There is no floating-point time anywhere in the engine.

### 4.2 Values and provenance

Every completed job writes one **value** to each of its outgoing edges:

```
Value { producer: NodeIdx, job: u64, written_at: Tick, provenance: Provenance }
```

**Provenance** records which sensor samples a value was computed from. It is indexed
by **chain** (a sensor→sink path, enumerated statically, formulation §3):

- a sensor job released at `r` writes `provenance[c] = r` for every chain `c` starting
  at that sensor. The release is the physical sampling instant. The sensor's WCET is
  driver / preprocessing time that happens after sampling;
- a task job writes, for each chain `c` that passes through it,
  `provenance[c] = provenance[c]` of the value it read on `c`'s incoming edge;
- if that edge was `optional` and empty, `provenance[c] = None`: chain `c` is
  *unavailable* in this output.

So a sink output knows, per chain, exactly how old the sensor data behind it is, with
no approximation.

For a `batch` read on an `all` edge (§4.5), the value read is the **newest** in the
batch (the integrator state is up to date as of its newest input).

> Chains can be exponential in number on adversarial DAGs. If the count exceeds a cap
> (default 4096) the simulator falls back to per-*source* provenance (oldest sample per
> sensor), which is the usual conservative data-age definition. Random-DAG experiments
> report which mode they used.

### 4.3 Edge channels

Each edge `u → v` is a channel whose behaviour depends on its policy:

| policy | buffer | on write | on read (job of `v` starts) |
|---|---|---|---|
| `latest` | register (1 slot) | overwrite | copy the value, **do not consume it**. Report it *stale* if `now − provenance > max_age` on any chain through the edge |
| `all` | unbounded FIFO | append | drain everything (batch). Depth is recorded for metrics |
| `optional` | register (1 slot) | overwrite | copy if present, else read "nothing" |

A value is **discarded** from an `all` edge only by an explicit `Discard` decision
(§6.2). Baselines like drop-oldest do this. The oracle flags it.

**Required inputs.** A job of `v` is *valid* only if every `latest` and `all` input has
ever been written. A job whose required inputs are missing (e.g. at start-up) completes
as **starved**: it takes no CPU time, writes nothing, and counts as a miss for `(m, k)`.
After warm-up this should not occur, and if it does it's reported.

### 4.4 Activation

The formulation (§9) leaves activation semantics open and §7.4 shows they dominate
feasibility, so the simulator supports all three and makes the choice explicit:

| `activation` value | meaning |
|---|---|
| `on=<pred>` | event-triggered: release a job each time predecessor `<pred>` writes a value |
| `periodic=<T>` (+ `offset`) | time-triggered every `T` ticks |
| `any` | release on every write to any input (ROS-style "any callback"); included mostly as the bad baseline from formulation §7.4 |

**Defaults:** a task with exactly one predecessor defaults to `on=<that pred>`. A task
with several predecessors **must** declare `activation`. That forces the §7.4 decision
to be made consciously instead of hidden in a default.

For `examples/pipeline.dot` the reference configuration is:

```dot
detect   [activation="on=cam"];
segment  [activation="on=cam"];
localize [activation="periodic=33000"];   // once per cam period, batches imu
fuse     [activation="on=detect"];
plan     [activation="on=fuse"];
actuate  [activation="on=imu", deadline=20000];
```

**Overlapping activations.** If a node is activated while its previous job is still
pending or running, the new job is released anyway (jobs of one node are queued in
order, never run concurrently). What to do with the backlog is a policy decision. That
is where "FIFO keeps stale work" vs. "drop-oldest" vs. the plan differ.

### 4.5 Jobs

A **job** is one activation of a node:

```
Job {
  node, index,              // index = activation count of this node, from 0
  release, abs_deadline,
  demand,                   // execution ticks needed, drawn from the trace
  executed,                 // ticks received so far
  state,
}
```

**Lifecycle**

```
            ┌─ Skip ──────────────────────────────► Skipped
activation ─┤
            └─ Admit ─► Ready ⇄ Running ─► Completed (on time | late)
                          │        │
                          │        └─ Abort ─► Aborted
                          └─ Cancel ─────────► Cancelled
```

| outcome | CPU used | writes outputs | counts as `(m, k)` hit? |
|---|---|---|---|
| Completed, on time | `demand` | yes | **yes** |
| Completed, late (after `abs_deadline`) | `demand` | yes | no |
| Skipped (never released) | 0 | no | no |
| Cancelled (released, never started) | 0 | no | no |
| Aborted (killed while running) | `executed` | no | no |
| Starved (§4.3) | 0 | no | no |

**Input read and output write.** Inputs are read when the job **first starts running**
(not at release). Outputs are written at **completion**. This is the "implicit
communication" model common to ROS 2 and AUTOSAR, and is what makes stale-input
cancellation meaningful: if the job waits in the ready queue, its inputs are still the
newest at the time it starts. A `let` option (read at release, write at
`release + period`; Logical Execution Time) is planned as a configuration switch for
comparison, but is not part of v1.

**Execution demand.** `demand = sample(exec model) × wcet`, scaled by any active
injection. For a job that drains `n > 1` values from `all` inputs,
`demand += (n − 1) × batch_wcet`. This is a modelling stand-in for the batch-WCET open
question (formulation §9). The simulator makes its effect measurable but doesn't
answer it.

**Budget overrun.** When `executed` reaches `wcet` and the job is not finished, the
engine raises `BudgetExhausted`. The policy may `Continue` or `Abort`. Overruns are
always recorded as monitor events, whatever the policy does.

### 4.6 Deadlines

- A job's **absolute deadline** is `release + D_n`, where `D_n` is the node's
  `deadline` attribute if given, otherwise its **activation period**: the sensor period
  for sensors, `T` for `periodic=T`, and the period of the nearest upstream sensor
  along the trigger edges for `on=<pred>`.
- The **end-to-end bound** is separate and is per chain: a sink output *violates the
  end-to-end bound on chain `c`* if its data age on `c` (§7.1) exceeds `D_c`. `D_c`
  defaults to the sink's `deadline`.

> **Note for the formulation.** §7.2 computes slack as if one release wave flows
> through the whole DAG within `D = 20 000`. Under the activation above, the C1 data
> age at `actuate` also includes waiting for `actuate`'s next imu activation (up to
> 5 000) and any ready-queue wait, so slack-from-WCET and measured data age don't line
> up exactly. The simulator will measure the gap. The formulation should say whether
> `D` bounds *response time of a release wave* or *data age at the sink*. This spec
> assumes data age, because that is what claim 1 is about.

### 4.7 Processor

- `m` identical cores (v1: `m = 1`), global scheduling, one job per core.
- **Preemptive** by default: when a higher-priority job becomes ready, it preempts the
  lowest-priority running job. Non-preemptive is a config switch.
- A job of a node never runs in parallel with another job of the same node.
- Optional constant `context_switch` cost charged when a core switches to a different
  job.

## 5. Engine

### 5.1 Structure

A classic discrete-event loop over a priority queue of pending events:

```
loop:
  (t, ev) = queue.pop_min()          // ordered by (time, phase, seq), §5.3
  if t > horizon: break
  advance running jobs to t           // credit executed ticks
  handle(ev)                          // may push new events, call policy hooks
  if phase boundary reached:
      dispatch()                      // policy chooses, engine assigns cores
      schedule next Completion / BudgetExhausted for each running job
```

The running job's next `Completion` / `BudgetExhausted` event is (re)computed on every
dispatch, and stale ones are invalidated by a per-job generation counter (standard
lazy-deletion trick), so preemption is O(log n).

### 5.2 Events

| event | source | effect |
|---|---|---|
| `SensorRelease(s, k)` | trace | release sensor job (subject to policy `on_activation`) |
| `PeriodicRelease(n, k)` | `periodic=T` activation | release task job |
| `Completion(job)` | engine | write outputs, record, trigger `on=` / `any` successors |
| `BudgetExhausted(job)` | engine | policy `on_overrun` → Continue / Abort |
| `DeadlineReached(job)` | engine | job becomes *late*; mark `(m, k)` miss now if still pending |
| `PolicyTimer(id)` | policy | e.g. hysteresis timer for stepping down a ladder level |
| `Horizon` | engine | stop. Pending jobs are reported, not counted as misses |

### 5.3 Tie-breaking at equal time (determinism)

Events at the same tick are processed in **phases**, in this order, and FIFO by
insertion sequence within a phase:

1. `Completion`. Values become visible at `t`.
2. `BudgetExhausted`, `DeadlineReached`.
3. `SensorRelease`. A sample taken at `t`.
4. `PeriodicRelease` and event-triggered releases caused by phases 1–3.
5. `PolicyTimer`.
6. **dispatch** (once per tick, after all of the above).

Consequence: a job that starts at `t` sees every value produced at or before `t`. An
event-triggered release happens at the same tick as the triggering completion, with
zero activation latency. That is a config option (`activation_latency`) for later.

### 5.4 Workload trace and RNG

The trace is generated before any policy runs and is the same for all policies within a
(scenario, seed):

- sensor release times: `k·T + offset + jitter_k`, with burst and drop injections applied;
- execution demand for `(node, job index)`: a lazily-generated, per-node stream.

Each node gets its own RNG stream, derived from `(seed, node name)` with ChaCha. Adding
a node or changing one node's distribution doesn't perturb the others' draws. Because
demand is indexed by *job index*, two policies that release the same `k`-th job of a node
see the same demand for it, even if they released it at different times.

Traces can be dumped and re-loaded (`--save-trace` / `--trace`), which gives exact
replay of an interesting run and lets measured traces drive the simulator.

## 6. Policies

### 6.1 Interface

A policy is a Rust trait object. It sees a read-only `SimView` (monitors, queues,
channel state, the plan) and answers a small number of hooks. Each hook returns a tiny
enum, so policy calls are cheap and the overhead is easy to measure.

```rust
pub trait Policy {
    fn name(&self) -> &str;

    /// A job is about to be released. Skip = never released.
    fn on_activation(&mut self, v: &SimView, job: JobId) -> Admit;          // Admit | Skip

    /// Called at each dispatch point: order the ready jobs and optionally prune them.
    /// The engine runs the top `cores` jobs of the returned order.
    fn dispatch(&mut self, v: &SimView, ready: &[JobId], out: &mut Decisions);
    //   out.order(job, key)    priority key, smaller runs first
    //   out.cancel(job)        drop a ready job
    //   out.discard(edge, n)   drop the n oldest values from an edge queue

    /// The job has used its whole WCET and is not finished.
    fn on_overrun(&mut self, v: &SimView, job: JobId) -> Overrun;           // Continue | Abort

    /// Any recorded event (completion, miss, stale read, ...), for monitor-driven policies.
    fn on_event(&mut self, v: &SimView, ev: &Event, timers: &mut Timers) {}
}
```

The engine only rejects decisions that are *physically* impossible (cancelling a job
that is already running, running two jobs of one node at once). It never rejects one
for being *unsafe*. That is the oracle's job (§6.3).

### 6.2 Policies to implement

| name | admit | order | prune | on overrun |
|---|---|---|---|---|
| `fifo` | all | release time | never | continue |
| `fp-rm` | all | activation period (rate-monotonic) | never | continue |
| `fp-crit` | all | (criticality, period) | never | continue |
| `edf` | all | `abs_deadline` | never | continue |
| `drop-oldest` | all | EDF | keep only the newest pending job per node, and cap every edge queue at `q` values (discarding oldest, *including on `all` edges*) | continue |
| `drop-late` | all | EDF | cancel any ready job past its deadline | abort |
| `plan` | ladder + `(m, k)` check | plan priority, boosted by chain slack and data age (formulation §6) | stale-input cancel on `latest` edges, when legal | per plan |

The baselines are deliberately simple and *unaware* of annotations except where noted:
they represent "a runtime without a degradation plan". `fp-crit` is the strongest
honest baseline: it knows criticality but has no plan.

**`plan` policy, concretely** (formulation §6):

- **Monitors** kept by the policy itself from `on_event`: per-node `(m, k)` history as a
  `k`-bit ring, per-chain remaining slack, per-edge age, overrun flags.
- **Escalation:** step up one ladder level when any chain's slack goes negative, or an
  overrun is detected on a node with `on_critical_path`. Step down one level after
  `hysteresis` ticks (config, default 5 × the longest sensor period) with no trigger.
- **Admit:** at level `L`, skip if the level's actions say so *and* the node's `(m, k)`
  history (and that of its drop cone, from the plan) stays valid after the skip.
  Otherwise admit.
- **Dispatch:** cancel a ready job whose `latest` input is older than `max_age` if the
  plan marks it `drop_legal` and the `(m, k)` check passes. Order by
  (effective criticality, remaining chain slack, input age).
- Every decision is a table lookup plus bit operations. No graph traversal at run time.

### 6.3 Oracle

The oracle consumes the event log (online or post-hoc; same code) and the annotated DAG
(**not** the plan, so it doesn't inherit the compiler's mistakes). It reports:

| violation | definition |
|---|---|
| `safety_drop` | a job of a node with effective criticality `safety` was skipped, cancelled, aborted, starved or completed late |
| `all_discard` | a value was removed from an `all` edge without being read |
| `mk_violation` | in some window of `k` consecutive jobs of `n`, fewer than `m` were on-time completions |
| `stale_read` | a job read a `latest` value older than the edge's `max_age` (counted, but a *quality* metric, not a contract) |
| `e2e_violation` | a sink output's data age on some chain exceeds `D_c` |

Effective criticality is recomputed inside the oracle with its own simple propagation
(formulation §4.2), independent of `rt-passes`. A disagreement between the two is
itself a test failure.

Each violation is attributed to the current ladder level, so "zero violations in
contract-preserving levels" is checkable directly.

## 7. Outputs and metrics

### 7.1 Definitions

- **Data age** of chain `c` at a sink completion at time `t`: `t − provenance[c]` of the
  value written. Undefined (reported as *unavailable*) if `provenance[c] = None`.
- **Reaction time** of chain `c` for a sensor sample taken at `r`: `t − r`, where `t` is
  the first sink completion with `provenance[c] ≥ r`. Computed post-hoc from the sorted
  list of sink outputs. Samples never reflected before the horizon are reported
  separately (*lost samples*), not dropped from the statistic silently.
- **Shed fraction:** CPU ticks of skipped, cancelled and aborted work (at their trace
  demand) ÷ CPU ticks of all released-or-skipped work.
- **Decision overhead:** wall-clock time spent inside policy hooks, measured with
  `Instant` around each call, reported as ns per call. Must be run in `--release`. It is
  a host measurement and only comparable across policies on the same machine.

### 7.2 Per-run outputs

Written to `output.dir/<scenario>/<policy>/seed-<n>/`:

| file | contents |
|---|---|
| `summary.json` | all scalar metrics below, plus the config hash and simulator version |
| `sink.csv` | one row per sink completion: time, level, per-chain age |
| `jobs.csv` | one row per job: node, index, release, start, finish, outcome, demand |
| `events.jsonl` | full event log (optional; it drives the oracle post-hoc and the viewer) |
| `trace.json` | the workload trace, when `--save-trace` |

**Summary metrics**

- data age per chain: mean, p50, p99, p99.9, max (after warm-up)
- reaction time per chain: same percentiles; lost-sample count
- deadline misses per node and per criticality class
- oracle violations by kind and by ladder level
- shed fraction, overall and per node; CPU utilisation
- time spent at each ladder level; number of level changes
- max and mean queue depth on `all` edges; max ready-queue length
- decision overhead: mean and p99 ns per hook

### 7.3 Aggregation

`rt-cli sim` over a config with several scenarios × policies × seeds writes one
`results.csv` with a row per run. Analysis and plotting happen outside the simulator
(Python / notebook). The simulator doesn't draw figures.

## 8. Crate layout and CLI

New crate `crates/rt-sim`, depending on `rt-schema`, `rt-passes`, `serde`,
`serde_yaml`, `serde_json`, `rand`, `rand_chacha`, `rand_distr`; dev-dependencies
`proptest`, `insta`.

```
rt-sim/src/
  lib.rs        // run(config) -> RunResult
  config.rs     // YAML experiment schema (§3.3)
  trace.rs      // workload generation, distributions, injections, save/load (§5.4)
  provenance.rs // chain enumeration, Provenance (§4.2)
  channel.rs    // latest / all / optional edges (§4.3)
  job.rs        // Job, lifecycle, outcomes (§4.5)
  engine.rs     // event queue, phases, dispatch, cores (§5)
  policy/       // trait + fifo, fp, edf, drop_oldest, drop_late, plan (§6)
  oracle.rs     // contract checks (§6.3)
  metrics.rs    // per-run metrics, CSV/JSON writers (§7)
```

CLI (in `rt-cli`):

```sh
rt-cli sim experiments/pipeline.yaml                       # full sweep
rt-cli sim examples/pipeline.dot --policy plan --horizon 1000000 --seed 1   # quick single run
rt-cli sim ... --save-trace t.json  /  --trace t.json      # record / replay a workload
rt-cli sim ... --gantt | ...                               # text Gantt of the first N ms, for debugging
```

## 9. Validation of the simulator itself

Before any result is trusted, the simulator has to be checked against cases with known
answers:

1. **Hand-traced micro-DAGs** (`const` execution times, 1 core): sensor → task;
   diamond with a `latest` join; `all` edge with batching; preemption between two
   periodic tasks; an `optional` edge going empty. Expected `jobs.csv` checked as
   snapshots.
2. **Schedulability sanity:** for implicit-deadline periodic task sets with `U ≤ 1`,
   `edf` has zero misses; for `U ≤ n(2^{1/n} − 1)`, `fp-rm` has zero misses.
3. **Utilisation:** with `const 1.0` demand and no shedding, measured CPU utilisation
   over a long horizon matches the analytical value (pipeline: ≈ 0.78, formulation §7.4)
   to within rounding.
4. **Determinism:** two runs with the same config and seed have byte-identical
   `events.jsonl`.
5. **Common random numbers:** two policies on the same (scenario, seed) see identical
   sensor release times and identical demand for every job index both release.
6. **Oracle self-test:** hand-made event logs with planted violations of each kind
   are all detected; a clean log reports none.
7. **Property test** (formulation §11): for random overrun traces (proptest), `plan`
   has zero `safety_drop`, `all_discard` and `mk_violation` while at contract-preserving
   levels.

### Acceptance scenario: formulation §7.7

On `examples/pipeline.dot` with the §7.5 annotations, the §4.4 activations, `const 1.0`
demand and `detect` scaled by 2.5× for a sustained window:

- `fifo` / `edf`: C1 data age grows over the window (backlog of stale camera frames);
- `drop-late`: bounded data age, but `mk_violation` on `plan` and/or `all_discard`-
  equivalent loss through aborted `localize` jobs;
- `plan`: steps to L2, C1 data age stays bounded, zero contract violations, and
  `actuate` never misses.

If the simulator can't reproduce this qualitative picture, either the simulator or the
formulation is wrong, and finding out which is the point.

## 10. Build plan

Each step ends with tests green and something runnable.

| step | content | depends on | done when |
|---|---|---|---|
| S1 | `rt-sim` skeleton: config, trace (`const`/`uniform`), engine with sensors + `on=` tasks, `latest` edges only, 1 core, `fifo` / `edf`, `jobs.csv` | `rt-schema` as-is | micro-DAG snapshots (§9.1), checks 2–4 pass |
| S2 | annotations in `rt-schema` + DOT (`policy`, `max_age`, `criticality`, `mk`, `activation`, `deadline`, `batch_wcet`) | formulation roadmap 1 | pipeline.dot with §7.5 annotations parses |
| S3 | all channel kinds, batching, `periodic` / `any` activation, provenance, data age + reaction time | S1, S2 | pipeline utilisation ≈ 0.78; age on a hand-traced chain is exact |
| S4 | oracle + remaining baselines (`fp-*`, `drop-oldest`, `drop-late`); injections, heavier distributions | S3 | oracle self-test; baselines show violations in the §7.7 scenario |
| S5 | `plan` policy, loading a **hand-written** plan JSON for pipeline.dot | S4 | acceptance scenario passes |
| S6 | swap in the `rt-passes` plan; property test | formulation roadmap 2 | property test passes; hand-written and generated plans agree |
| S7 | sweep runner, `results.csv`, random DAG generator (formulation roadmap 5) | S6 | experiment configs in `experiments/` reproduce the paper figures |

S1–S5 don't need `rt-passes` at all. That lets the simulator and the analyses be
built in parallel, and the hand-written plan doubles as the expected output for the
`rt-passes` tests.

## 11. Decisions to confirm

These are the choices this spec makes that most affect results. Each has a default
above. Change them here before S1 if you disagree.

1. **Read at start, write at completion** (implicit communication) rather than LET.
   LET makes data age deterministic but hides exactly the queueing staleness the
   formulation targets.
2. **Multi-input tasks must declare `activation`**, with no silent default.
3. **The end-to-end bound `D` bounds data age at the sink**, not wave response time
   (§4.6 note).
4. **Batch read takes the newest value's provenance** for `all` edges.
5. **Late completions count as `(m, k)` misses** (firm semantics). The alternative,
   "any completion counts", is weaker and would make baselines look better.
6. **Uniprocessor for v1 experiments**, with the engine already supporting `m` cores.
7. **The oracle uses annotations, not the plan**, so compiler bugs are caught rather
   than inherited.
