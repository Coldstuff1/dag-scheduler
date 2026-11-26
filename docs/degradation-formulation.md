# Degradation plans: criticality- and freshness-aware load shedding for `rt` DAGs

Status: problem formulation / research note. Nothing here is implemented yet.

## 1. Problem

An `rt` program is a multi-rate DAG: periodic sensors feed tasks, which converge on a
single sink (see the [README](../README.md)). At design time every node has a WCET,
but at run time execution drifts from that model. Tasks overrun, sensors jitter, and
queues build up. When that happens the system has to drop something. Without guidance a
runtime drops whatever happens to be late, and that has two failure modes:

- **Too conservative**: it finishes stale work (a 3-frame-old camera image) because it
  was already queued, so outputs get older exactly when the system is struggling.
- **Too aggressive**: it drops something that looked unimportant but that safety or
  correctness depends on (an IMU sample an integrator needed, or a job on the reflex path).

**Goal:** a compile-time process that extracts the structure of the DAG that matters for
degradation, and turns it into a *plan* the runtime follows when observed execution
deviates from expectations. The plan should
1. improve **freshness** (end-to-end data age at the sink) under overload, and
2. **provably never** shed work that safety-critical outputs depend on, nor exceed any
   node's declared skip budget.

## 2. Core idea

> Compile the DAG into a **degradation plan**: a static table of which jobs and inputs
> may be dropped, what dropping each one invalidates, and what it buys. The runtime then
> maps observed deviations to O(1) shedding and priority decisions that stay above a
> provable safety floor.

The key design choice is a strict split:

| | Static (compiler, `rt-passes`) | Dynamic (runtime) |
|---|---|---|
| does | derive structure, check legality, precompute options | observe, choose among precomputed options |
| never | depends on run-time state | invents a drop the compiler didn't approve |

All "is this safe to drop?" reasoning happens offline. The runtime stays small, fast and
analysable, and safety follows by construction: the runtime can only select legal actions.

## 3. What structure alone tells you

These properties come from the DAG as it is today, with no new annotations.

| Structure | Computed by | Why the runtime cares |
|---|---|---|
| **Cause-effect chains** (every sensor→sink path) | path enumeration (or DP over topo order) | the unit for end-to-end *data age* and *reaction time* |
| **Critical path and slack** per node | longest path on WCET, earliest/latest finish against a sink deadline | slack is how much overrun a node absorbs before the sink is late |
| **Post-dominators** | post-dominator tree, which is well defined because there's exactly one sink | choke points: an overrun there delays everything downstream. Shed *before* them, not at them |
| **Joins with rate skew** | propagate each node's set of source periods forward | where fast and slow data meet, the slow input is usually stale. Prime place to drop or hold |
| **Rate transitions / oversampling** | effective activation rate vs. the consumer's rate | a node running faster than anything downstream consumes it can skip jobs at little or no cost |
| **Parallel / redundant branches** | sibling paths with a common source and a common join | candidates for best-effort work |
| **Bypass (reflex) paths** | a short sensor→sink path alongside a long one from the same sensor | the natural safety floor: keep the short path while shedding the long one |

## 4. What structure cannot tell you: minimal annotations

Topology can't tell an IMU integrator, which must see *every* sample, from an object
detector, which only wants the *newest* frame. That distinction is the whole
freshness-vs-correctness question, so we add a few annotations. Everything else is
derived.

### 4.1 Edge consumption policy (per `rt.task` operand)

| policy | meaning | may the runtime discard values? |
|---|---|---|
| `latest` | consumer wants the newest value, and it must be younger than the edge's `max_age` | yes: overwrite older values. Dropping *improves* freshness |
| `all` | consumer processes every value, in order (integrators, filters, counters) | **never.** Jobs may be *deferred* and values batched, but not discarded |
| `optional` | consumer may run with a held (stale) value, or none | yes, and dropping the producer doesn't block the consumer |

`latest` and `all` edges are **required**: the consumer can't fire validly without them.
`optional` edges are not.

### 4.2 Node criticality

`safety > mission > best_effort`, in the mixed-criticality sense. Users mark only the
obvious nodes, usually the sink or the reflex path. A pass propagates the level
**upstream along required edges**. If a `safety` node requires `x`, then `x` is at least
`safety`. Propagation stops at `optional` edges, which is what lets a safety-critical
sink tolerate a mission-level planner being shed.

### 4.3 Timing contracts

- **Sink deadline** `D`: end-to-end bound used to compute slack.
- **`max_age`** on `latest` edges, or per chain: the freshness bound.
- **Weakly-hard `(m, k)`** per node: in any `k` consecutive jobs, at least `m` complete.
  `safety` defaults to `(k, k)`, i.e. never skip. `best_effort` defaults to `(0, k)`.

`(m, k)` turns "may be dropped" into a bounded, checkable contract. The runtime keeps a
k-bit completion history per node, and a skip is legal only if the history stays valid.
It also **composes**: skipping `n` also skips every node in `n`'s *drop cone* (§5) for
that activation, so a skip of `n` spends budget on all of them. The static pass checks
that the budgets are consistent. A node can't promise to skip more often than its
cone's strictest downstream contract allows, unless a `max_age` bound lets consumers
reuse the previous value (see the worked example).

### 4.4 Drop legality, in one rule

Dropping a job of node `n` (or a value on edge `e`) is **legal** iff

1. no value is discarded from an `all` edge;
2. no node in the drop cone has effective criticality `safety`; and
3. every node in the drop cone (including `n`) still meets its `(m, k)` after the skip.

Conditions 1 and 2 are fully static. For condition 3, the compiler precomputes the
cone and the strictest budget, so the runtime check is a few bit operations.

## 5. Compile-time output: the degradation plan

Per node:

- `slack`, `on_critical_path`, `post_dominators`, `chains`
- `effective_criticality` (after propagation)
- `drop_cone`: downstream closure through required edges whose freshness bound would be
  violated by the skip
- `drop_legal`, `skip_budget` (effective `(m, k)` after composing over the cone)
- `drop_gain`: WCET (utilisation) freed by the skip, including the cone
- `skip_ratio` hint for oversampled nodes

Plus a short **degradation ladder** `L0 … Ln`: L0 runs everything, and each level sheds
the next legal set ranked by gain per unit of criticality lost. Levels that stay within
every `(m, k)` are *contract-preserving*. A final level may be a mixed-criticality
**mode switch** that suspends `mission` contracts and keeps only the `safety` floor.

The plan would be attached to the IR as `rt.analysis.*` attributes (visible and
snapshot-testable) and exported as a flat table (e.g. JSON) for the runtime.

## 6. Runtime: expectations → decisions

**Monitors**, which are cheap and updated per job:
- execution time vs. budget (WCET), and vs. typical time as an early warning
- remaining slack along each chain
- data age at each join and at the sink
- `(m, k)` histories
- queue depth on `latest` edges

**Policy:**
- **Stale-input dropping (freshness).** On a `latest` edge, keep only the newest value.
  If a pending job's input is already older than `max_age`, cancel it and wait for fresh
  data instead of finishing stale work, provided the cancellation is legal.
- **Level escalation (overload).** If chain slack goes negative or an overrun is
  detected, step to the next ladder level. Step back down with hysteresis once slack
  recovers.
- **Priority adjustment.** Order by effective criticality first, then least remaining
  chain slack, then input data age. When upstream ran long, boost critical-path and
  post-dominator nodes.
- **Invariant.** `safety` nodes, `all`-edge values and `(m, k)` contracts are never
  violated in contract-preserving levels. This holds by construction, because the
  runtime only picks among precomputed legal actions.

## 7. Worked example: `examples/pipeline.dot`

```
cam   (wcet 200,  T 33 ms)  ──► detect  (8000) ──┐
                            └─► segment (6000) ──┤
lidar (wcet 150,  T 100 ms) ──► localize (3000) ─┼─► fuse (2500) ─► plan (4000) ─► actuate (300)
imu   (wcet 20,   T 5 ms)   ──► localize          │                                  ▲
                            └────────────────────────────────────────────────────────┘
```

Times in µs. Assumed sink deadline `D = 20 000` µs.

### 7.1 Chains

| # | chain | Σ WCET |
|---|---|---|
| C1 | cam → detect → fuse → plan → actuate | **15 000** (critical) |
| C2 | cam → segment → fuse → plan → actuate | 13 000 |
| C3 | lidar → localize → fuse → plan → actuate | 9 950 |
| C4 | imu → localize → fuse → plan → actuate | 9 820 |
| C5 | imu → actuate | 320 (**bypass**) |

### 7.2 Slack

Earliest finish (EF) / latest finish (LF) against `D`, assuming unlimited parallelism:

| node | EF | LF | slack |
|---|---|---|---|
| cam | 200 | 5 200 | 5 000 |
| lidar | 150 | 10 200 | 10 050 |
| imu | 20 | 10 200 | 10 180 |
| detect | 8 200 | 13 200 | 5 000 |
| segment | 6 200 | 13 200 | 7 000 |
| localize | 3 150 | 13 200 | 10 050 |
| fuse | 10 700 | 15 700 | 5 000 |
| plan | 14 700 | 19 700 | 5 000 |
| actuate | 15 000 | 20 000 | 5 000 |

The critical path C1 has the minimum slack (`D − 15 000`). On a single core, these
numbers would come from response-time analysis instead, but the ranking logic is the same.

### 7.3 Post-dominators

| node | post-dominators | immediate |
|---|---|---|
| cam | fuse, plan, actuate | fuse |
| lidar | localize, fuse, plan, actuate | localize |
| imu | **actuate only** | actuate |
| detect, segment, localize | fuse, plan, actuate | fuse |
| fuse | plan, actuate | plan |
| plan | actuate | actuate |

`fuse → plan → actuate` is the choke spine: an overrun there hits every chain. `imu`'s
only post-dominator is the sink, which is how the bypass is detected structurally.
Of imu's two paths, one is 320 µs and the other 9 820 µs.

### 7.4 Joins and rates

| join | inputs (source periods) | skew |
|---|---|---|
| localize | lidar 100 ms, imu 5 ms | 20× |
| fuse | {cam 33 ms}, {cam 33 ms}, {lidar 100, imu 5 ms} | mixed |
| actuate | plan (mixed), imu 5 ms | mixed |

**Oversampling.** If tasks fired on *any* input arrival, imu's 5 ms rate would propagate
to localize, fuse and plan. That gives utilisation 3000/5000 + 2500/5000 + 4000/5000 ≈ 1.9
for those three alone, which is infeasible on one core. The rate analysis flags that
fuse's output only matters at roughly the cam rate. So localize should run once per fuse
activation and process the ~6.6 queued imu samples as a **batch**. That respects the
`all` edge, since every sample is consumed, without running every job. (Open question:
the batch WCET needs its own bound.)

With tasks activated at 33 ms, apart from actuate on imu's 5 ms reflex rate, nominal
utilisation is

```
sensors 0.012 + detect 0.242 + segment 0.182 + localize 0.091
        + fuse 0.076 + plan 0.121 + actuate 0.060  ≈ 0.78
```

### 7.5 Annotations

```dot
cam      -> detect   [policy=latest, max_age=70000];   // ~2 cam periods
cam      -> segment  [policy=latest];
lidar    -> localize [policy=latest, max_age=150000];
imu      -> localize [policy=all];                     // integrator
detect   -> fuse     [policy=latest, max_age=70000];
segment  -> fuse     [policy=optional];                // fuse degrades without it
localize -> fuse     [policy=latest];
fuse     -> plan     [policy=latest];
plan     -> actuate  [policy=optional];                // actuate holds last plan
imu      -> actuate  [policy=latest];
actuate  [criticality=safety];
plan     [criticality=mission, mk="3,4"];
```

(DOT syntax sketch: attribute names are not final. In MLIR, edge policies would be a
`policies` array on `rt.task` aligned with its operands.)

**Propagated criticality:**

| node | effective | why |
|---|---|---|
| actuate, imu | safety | marked / required by actuate (imu → actuate is `latest`) |
| plan, fuse, detect, localize, cam, lidar | mission | required chain into plan. `plan → actuate` is `optional`, so safety stops there |
| segment | best_effort | only reaches fuse through an `optional` edge |

So the safety floor is exactly the reflex path `imu → actuate`, derived from two
annotations.

### 7.6 Drop cones and legality

| skip one job of | drop cone | legal? | gain (U) |
|---|---|---|---|
| segment | ∅ (optional edge) | yes, any rate | 0.182 |
| detect | ∅ for a single skip (fuse reuses the prior detection, age ≤ 66 ms < 70 ms). {fuse, plan} for two in a row | yes, at most 1 of every 2. Plan's (3,4) is not touched | 0.121 at half rate |
| localize | {fuse, plan} for that cycle. imu samples are *deferred* to the next batch | yes, if plan's (3,4) allows | 0.091 |
| fuse / plan | {plan} / ∅ | only within plan's (3,4) | 0.076 / 0.121 |
| imu, actuate | n/a | **never** (safety) | n/a |

The `max_age` on `detect → fuse` lets detect skip without spending plan's budget.
Loosening a freshness bound directly buys shedding headroom. That trade-off is explicit
and analysable, which is the point.

### 7.7 Degradation ladder

| level | sheds | U | contracts |
|---|---|---|---|
| L0 | nothing | 0.78 | all |
| L1 | segment | 0.60 | all preserved |
| L2 | L1 + detect at half rate | 0.48 | all preserved |
| L3 (mode switch) | everything but imu → actuate, which holds the last plan | 0.06 | `mission` contracts suspended, safety kept |

**Scenario:** detect overruns at 2.5× WCET (20 000 µs). Utilisation goes to ≈ 1.15 and
the system is overloaded.
- L1 gives ≈ 0.97, which is barely feasible with no margin, so keep climbing.
- L2 gives ≈ 0.66. Plan still meets (3,4), fuse sees detections at most 66 ms old, and
  actuate never misses.

A naive runtime would instead queue late camera frames (staleness grows without bound),
or drop whichever job is late. That could be localize, which loses imu samples and
corrupts the integrator.

## 8. Research framing

**Question.** Can compile-time degradation plans derived from DAG structure and data-flow
semantics let a runtime improve end-to-end freshness under overload, while provably
preserving safety-critical chains and weakly-hard contracts, at lower runtime cost than
online analysis?

**Claims to test:**
1. Lower sink data age and reaction time under overload than no-shedding (FIFO, EDF)
   and naive drop-oldest or drop-late baselines.
2. Zero violations of `safety` chains, `all` edges and `(m, k)` in contract-preserving
   levels. Naive shedding is expected to violate these.
3. Decision overhead is O(1) per event, since the plan is a lookup.

**Related work to position against:**
- end-to-end analysis of cause-effect chains (data age, reaction time), e.g. Becker et
  al., Dürr et al.
- mixed-criticality scheduling (Vestal; the Burns & Davis survey)
- weakly-hard / (m,k)-firm real-time (Hamdaoui & Ramanathan; Bernat, Burns & Llamosí)
- imprecise and anytime computation (Liu et al.); elastic task model (Buttazzo et al.)
- Age-of-Information scheduling
- ROS 2 executor and processing-chain analysis (e.g. Casini et al.)
- Logical Execution Time

*Gap to argue:* prior work mostly either *analyses* chains or *sheds* load. Deriving drop
legality from the DAG's data-flow semantics (`latest`/`all`/`optional`), composing
`(m, k)` across drop cones, and compiling the result into a runtime table is the
proposed contribution.

## 9. Open questions

- **Activation semantics.** Is a task time-triggered, triggered by one designated input,
  or triggered by any input? §7.4 shows this choice dominates feasibility. It may need to
  be an annotation too.
- **Batch WCET** for jobs that drain an `all` queue after deferral.
- **Multicore.** Slack and ladder gains above are uniprocessor and unlimited-parallelism
  approximations.
- **Choosing levels.** Greedy gain-per-criticality ordering vs. an optimisation
  (e.g. ILP over legal shed sets).
- **Expectation models.** WCET-only budgets vs. learned typical-case times for early
  warning (overrun prediction before the budget is exhausted).
- **Where the plan lives.** MLIR attributes vs. a separate runtime artefact, and whether
  analyses run on `Dag` or as MLIR passes over `!rt.token` use-def chains.

## 10. Implementation roadmap

1. **Model and dialect**: criticality, `(m, k)`, sink `deadline` on nodes; `policy` and
   `max_age` on edges. `rt-schema` model plus DOT frontend; optional attributes in IRDL
   with a `policies` array on `rt.task`; `verify_rt` checks it matches the operand count.
2. **Analyses (`rt-passes`)**: chains → slack → post-dominators → rate propagation →
   criticality propagation → drop cones and legality → ladder. Build on
   `Dag::predecessors`, `Dag::topo_order` and `Dag::sink`.
3. **Emit / CLI**: `rt-cli analyze <file.dot>` prints the plan. JSON export. `viz`
   colours nodes by criticality and marks droppable ones.
4. **Simulator (`rt-sim`)**: discrete-event, periodic sensors, policy-defined
   activation, pluggable schedulers (baselines vs. plan-driven).
5. **Experiments**: random DAG generator with several rate mixes; heavy-tailed overrun
   and bursty-sensor injection.

## 11. Evaluation plan

- Unit tests per analysis against the hand-computed tables in §7.
- Snapshot of `rt-cli analyze examples/pipeline.dot`, following the existing import
  snapshot pattern.
- Property test in the simulator: for any injected overrun trace, the plan-driven
  scheduler never drops a `safety` job or an `all`-edge value, and never violates
  `(m, k)` at contract-preserving levels.
- Metrics: sink data-age distribution, deadline misses per criticality class, fraction
  of work shed, decision overhead.
