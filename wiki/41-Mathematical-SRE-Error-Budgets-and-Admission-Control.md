# 41 — Mathematical SRE: Error Budgets & Admission Control

> **Corresponding Specifications:** [`sys-arch/101-anonymous-network-capacity-management-load-shedding-admission-control-autoscaling-resource-fairness-privacy-preserving-availability-architecture.md`](../sys-arch/101-anonymous-network-capacity-management-load-shedding-admission-control-autoscaling-resource-fairness-privacy-preserving-availability-architecture.md), [`sys-arch/102-anonymous-network-cost-governance-resource-accounting-budget-enforcement-capacity-economics-privacy-preserving-finops-architecture.md`](../sys-arch/102-anonymous-network-cost-governance-resource-accounting-budget-enforcement-capacity-economics-privacy-preserving-finops-architecture.md), [`sys-arch/109-anonymous-network-service-level-objectives-error-budgets-reliability-policy-availability-governance-privacy-preserving-reliability-engineering-architecture.md`](../sys-arch/109-anonymous-network-service-level-objectives-error-budgets-reliability-policy-availability-governance-privacy-preserving-reliability-engineering-architecture.md), [`sys-arch/110-anonymous-network-availability-modeling-fault-domains-redundancy-planning-failure-correlation-reliability-simulation-privacy-preserving-resilience-engineering-architecture.md`](../sys-arch/110-anonymous-network-availability-modeling-fault-domains-redundancy-planning-failure-correlation-reliability-simulation-privacy-preserving-resilience-engineering-architecture.md), [`sys-arch/112-anonymous-network-performance-budgeting-latency-decomposition-throughput-modeling-tail-latency-control-privacy-preserving-performance-engineering-architecture.md`](../sys-arch/112-anonymous-network-performance-budgeting-latency-decomposition-throughput-modeling-tail-latency-control-privacy-preserving-performance-engineering-architecture.md)  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§2.16), [Wiki Chapter 31](31-SRE-Physical-Security-and-Operations.md)

---

## 1. Quantitative SRE Foundations for Sovereign Networks

Decentralized, privacy-preserving networks cannot afford operational guesswork. An uncontrolled traffic burst or distributed denial-of-service (DDoS) attack can cause cascading memory exhaustion across mixnet nodes and mailbox gateways, turning a resilient mesh into an unresponsive blackhole.

In Specs 101–112, SIAR replaces ad-hoc monitoring with **Rigorous Mathematical Site Reliability Engineering**:
- Network queues are modeled using formal queueing theory ($M/M/1/K$, $M/M/c/K$, and Little's Law).
- Service availability is governed by mathematical Error Budgets calculated over rolling 30-day windows.
- Inbound traffic surges trigger deterministic, multi-tier load shedding that preserves 100% capacity for life-safety SOS frames.

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                         MATHEMATICAL RELIABILITY PIPELINE                              │
├────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                        │
│ [Inbound Traffic Surge: Lambda Packets/Sec]                                            │
│                     │                                                                  │
│                     ▼                                                                  │
│ [Queue Admission Evaluator: M/M/c/K Buffer Saturation Check]                           │
│                     │                                                                  │
│     ┌───────────────┴───────────────┐                                                  │
│     │ (Buffer Occupancy < 80%)      │ (Buffer Occupancy >= 80%)                        │
│     ▼                               ▼                                                  │
│ [Normal Ingest]             [Priority-Tiered Load Shedding Token Bucket]               │
│                               ├── P0 (SOS Beacons): 100% Ingested Preemptively         │
│                               ├── P1 (Direct Messages): Rate-Limited Ingest            │
│                               ├── P2 (Media Attachments): Deferred to Disk             │
│                               └── P3 (Decoy Cover Traffic): 100% Dropped Instantly     │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Queueing Theory Foundations & Multi-Server $M/M/c/K$ Buffers

Every mixnet forwarder buffer is modeled as an **$M/M/c/K$ finite-capacity queue**, where packets arrive with Poisson rate $\lambda$, $c$ worker threads process cells with exponential rate $\mu$, and maximum buffer capacity is $K$ packets.

### Traffic Intensity ($\rho$)
$$\rho = \frac{\lambda}{c \mu}$$

### Packet Drop Probability ($P_{\text{drop}}$)
When the buffer reaches capacity $K$, arriving packets drop:

$$P_{\text{drop}} = P_K = \frac{\frac{(c\rho)^K}{c! c^{K-c}} P_0}{\sum_{n=0}^{c-1} \frac{(c\rho)^n}{n!} + \sum_{n=c}^K \frac{(c\rho)^n}{c! c^{n-c}}}$$

### Erlang-C Waiting Probability & Tail Latency Distribution
The probability that an arriving admitted packet must wait in the queue is given by the Erlang-C formula $C(c, \rho)$:

$$P(\text{Wait} > t) = C(c, \rho) \cdot e^{-c\mu(1 - \rho)t}$$

To prevent tail latency blowups while satisfying the $1,500\text{ ms}$ Poisson delay budget, node capacity is engineered such that $\rho \le 0.70$ under peak operating loads.

---

## 3. Mathematical Formulation of Service Level Objectives (SLOs)

In [`sys-arch/109`](../sys-arch/109-anonymous-network-service-level-objectives-error-budgets-reliability-policy-availability-governance-privacy-preserving-reliability-engineering-architecture.md), reliability is quantified over a rolling 30-day window:

$$\text{SLI} = \frac{\sum_{i=1}^N \mathbb{I}(\text{Request}_i \text{ is Successful})}{\text{Total Valid Requests}} \ge \text{SLO}$$

### Error Budget Equation
$$\text{Error Budget} = 1.0 - \text{SLO}$$

For SIAR's core mixnet packet forwarding engine ($\text{SLO} = 99.95\%$):

$$\text{Error Budget} = 1.0 - 0.9995 = 0.0005 \quad (0.05\% \text{ allowable failure})$$

### Multi-Window Multi-Burn-Rate Alerting
To detect sudden critical outages within minutes without generating pager fatigue during minor transient blips:

$$\text{BurnRate} = \frac{\text{Budget Consumed Fraction}}{\text{Time Elapsed Fraction}} = \frac{\Delta \text{Budget} / \text{TotalBudget}}{\Delta t / 30\text{ days}}$$

| Severity | Time Window ($\Delta t$) | Burn Rate Factor | Budget Consumed | Pager Response Window |
| :--- | :--- | :--- | :--- | :--- |
| **P0 Emergency** | 1 Hour (1h) | $14.4\times$ | $2.0\%$ of 30-day budget | Page on-call immediately ($< 5\text{ mins}$) |
| **P1 Critical** | 6 Hours (6h) | $6.0\times$ | $5.0\%$ of 30-day budget | Page on-call immediately ($< 15\text{ mins}$) |
| **P2 Warning** | 3 Days (72h) | $1.0\times$ | $10.0\%$ of 30-day budget | Non-urgent ticket (Next business day) |

**Automated Deployment Freeze Policy**: If $> 50\%$ of the monthly error budget burns in any 7-day period, automated canary rollouts freeze across the cluster until root-cause remediations are verified.

---

## 4. Priority-Tiered Load Shedding Algorithm (`sys-arch/101`)

```rust
pub struct AdmissionController {
    max_buffer_capacity: usize,
    current_occupancy: usize,
    p1_token_bucket: TokenBucket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePriority {
    P0_LifeSafety,
    P1_DirectMessage,
    P2_BulkMedia,
    P3_DecoyCover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    AdmitPreemptive,
    Admit,
    SpillToDiskQueue,
    Defer,
    DropSilently,
}

impl AdmissionController {
    pub fn evaluate_admission(&mut self, frame_priority: FramePriority) -> AdmissionDecision {
        let saturation_ratio = self.current_occupancy as f32 / self.max_buffer_capacity as f32;

        match frame_priority {
            FramePriority::P0_LifeSafety => AdmissionDecision::AdmitPreemptive,
            FramePriority::P1_DirectMessage => {
                if saturation_ratio < 0.90 && self.p1_token_bucket.consume(1) {
                    AdmissionDecision::Admit
                } else {
                    AdmissionDecision::SpillToDiskQueue
                }
            }
            FramePriority::P2_BulkMedia => {
                if saturation_ratio < 0.75 {
                    AdmissionDecision::Admit
                } else {
                    AdmissionDecision::Defer
                }
            }
            FramePriority::P3_DecoyCover => {
                if saturation_ratio < 0.60 {
                    AdmissionDecision::Admit
                } else {
                    AdmissionDecision::DropSilently
                }
            }
        }
    }
}
```

---

## 5. Latency Decomposition & Hedged Requests (`sys-arch/112`)

Transit latency through the 3-layer mixnet is strictly budgeted:

$$T_{\text{end-to-end}} = T_{\text{ingress}} + \sum_{i=1}^3 \left(T_{\text{crypto}, i} + T_{\text{poisson}, i} + T_{\text{tx}, i}\right) + T_{\text{egress}}$$

```text
┌───────────────────────────────────────────────────────────────────────────────┐
│ Metric Dimension              │ p50 Median   │ p99 Tail Limit │ Mitigation    │
├───────────────────────────────┼──────────────┼────────────────┼───────────────┤
│ Per-Hop Sphinx Peeling        │ 0.45 ms      │ 1.80 ms        │ AVX-512 SIMD  │
│ Poisson Mixing Delay (per hop)│ 450 ms       │ 2,400 ms       │ Dynamic Lambda│
│ Wire Transmission (QUIC/UDP)  │ 8.5 ms       │ 45.0 ms        │ BBR Congestion│
│ Mailbox Egress Fetch          │ 12.0 ms      │ 65.0 ms        │ redb KV Read  │
└───────────────────────────────────────────────────────────────────────────────┘
```

### Tail Latency Control: Hedged Requests
To eliminate $p99.9$ latency outliers caused by intermittent network hiccups, the client dispatches a second redundant Sphinx query through an alternate Layer 1 mix if no delivery receipt arrives within $2 \times \text{sRTT} + 1,500\text{ ms}$.

---

## 6. Threat Vectors & Resource Exhaustion Defenses

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        SRE RELIABILITY THREAT & DEFENSE MATRIX                         │
├────────────────────────┬─────────────────────────┬─────────────────────────────────────┤
│ Threat Vector          │ Attack Mechanism        │ SIAR Defense Architecture           │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Queue Flood DDoS**   │ Inundating mix nodes    │ Multi-tier load shedding; P3 cover  │
│                        │ with millions of cells  │ traffic shed instantly to clear RAM.│
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Algorithmic Attack** │ Sending malformed frames│ Constant-time cryptographic checks; │
│                        │ forcing CPU loops       │ parsing memory capped at 64KB.      │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Pager Storm DoS**    │ Inducing micro-spikes to│ Multi-window multi-burn-rate math   │
│                        │ fatigue on-call team    │ eliminates false positive pages.    │
└────────────────────────┴─────────────────────────┴─────────────────────────────────────┘
```

---

## 7. Erlang-C Tail Distribution & Queuing Calculus

To rigorously size CPU worker thread pools across sovereign mix nodes handling stochastic packet arrivals, SIAR applies the **Erlang-C $M/M/c/K$ Multi-Server Model**:

### 7.1. Erlang-C Waiting Probability
Let $\lambda$ be the Poisson arrival rate, $\mu$ be the service rate per CPU worker core, and $c$ be the number of worker cores. Offered traffic intensity is $a = \lambda / \mu$ and server utilization is $\rho = a / c < 1$.

The probability that an arriving cell must wait in the buffer is:

$$P(\text{Wait}) = C(c, a) = \frac{\frac{a^c}{c!} \cdot \frac{1}{1 - \rho}}{\sum_{k=0}^{c-1} \frac{a^k}{k!} + \frac{a^c}{c!} \cdot \frac{1}{1 - \rho}}$$

### 7.2. Tail Latency Distribution $P(W > t)$
The probability that packet waiting time $W$ exceeds threshold $t$ follows an exponential tail:

$$P(W > t) = C(c, a) \cdot e^{-(c\mu - \lambda) t}$$

To enforce the $p99.9$ latency invariant ($P(W > 10\text{ ms}) \le 0.001$), mix nodes dynamically scale worker thread allocations $c$ such that:

$$c \ge \frac{\lambda}{\mu} + \frac{\ln(1000 \cdot C(c, a))}{10\text{ ms} \cdot \mu}$$

---

## 8. Multi-Window Multi-Burn-Rate Alerting Architecture

Traditional threshold alerting (e.g. alert if error rate $> 1\%$ for 5 minutes) produces disastrous alert fatigue during brief transient blips while missing slow, continuous error leaks that completely exhaust quarterly error budgets. SIAR mandates **Google SRE Multi-Window Multi-Burn-Rate Policies**:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        MULTI-WINDOW ERROR BUDGET BURN POLICIES                         │
├─────────────┬───────────┬──────────────┬──────────────┬────────────────────────────────┤
│ Severity    │ Burn Rate │ Long Window  │ Short Window │ Budget Consumed Before Alert   │
├─────────────┼───────────┼──────────────┼──────────────┼────────────────────────────────┤
│ **Page P0** │ 14.4x     │ 1 hour (2%)  │ 5 min (0.16%)│ 2.0% in 1 hour                 │
│ **Page P1** │ 6.0x      │ 6 hours (5%) │ 30 min (0.4%)│ 5.0% in 6 hours                │
│ **Ticket**  │ 1.0x      │ 3 days (10%) │ 6 hours (0.8%)│ 10.0% in 3 days               │
└─────────────┴───────────┴──────────────┴──────────────┴────────────────────────────────┘
```

Both long and short windows must burn simultaneously before triggering an alert:
- **Alert Invariant**: $\text{BurnRate}_{\text{long}} \ge B \land \text{BurnRate}_{\text{short}} \ge B$.
- **Fast Reset**: Once a blip passes, the short window drops below $B$ within 5 minutes, automatically resolving the page without human intervention.

---

## 9. Controlled Delay (CoDel) Active Queue Management (AQM)

Traditional tail-drop queues cause bufferbloat, holding packets in RAM for hundreds of milliseconds and starving real-time media streams. SIAR implements **CoDel AQM** directly on all network egress sockets:

```text
Incoming Packet
       │
       ▼ Measure Sojourn Time: T_sojourn = T_dequeue - T_enqueue
Is T_sojourn > Target (5 ms) for Duration > Interval (100 ms)?
       ├── NO  ──> Forward Packet Normally (Buffer Healthy)
       └── YES ──> Enter DROPPING State!
                   │
                   ▼ Drop Packets with Rate Scaling:
                   NextDropTime = Now + Interval / sqrt(DropCount)
```

By scaling drop intervals inversely with the square root of consecutive drops ($t_{\text{next}} = t + \frac{100\text{ ms}}{\sqrt{n}}$), CoDel quickly drains standing buffer queues while leaving bursty, well-behaved traffic unmolested.

---

## 10. Production Rust CoDel Active Queue Manager

The following implementation in [`crates/siar-routing-policy`](../crates/siar-routing-policy) enforces Controlled Delay queue management across packet forwarding pipelines:

```rust
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct EnqueuedFrame {
    pub payload: Vec<u8>,
    pub enqueued_at: Instant,
}

pub struct CoDelQueueManager {
    target: Duration,
    interval: Duration,
    queue: VecDeque<EnqueuedFrame>,
    first_above_time: Option<Instant>,
    drop_next: Instant,
    count: u32,
    is_dropping: bool,
}

impl CoDelQueueManager {
    pub fn new(target: Duration, interval: Duration) -> Self {
        Self {
            target,
            interval,
            queue: VecDeque::with_capacity(1024),
            first_above_time: None,
            drop_next: Instant::now(),
            count: 0,
            is_dropping: false,
        }
    }

    pub fn enqueue(&mut self, payload: Vec<u8>) {
        self.queue.push_back(EnqueuedFrame {
            payload,
            enqueued_at: Instant::now(),
        });
    }

    /// Dequeues the next frame, dropping packets if sojourn time exceeds target threshold
    pub fn dequeue(&mut self) -> Option<Vec<u8>> {
        let now = Instant::now();

        while let Some(frame) = self.queue.pop_front() {
            let sojourn_time = now.duration_since(frame.enqueued_at);

            if sojourn_time < self.target {
                self.first_above_time = None;
                return Some(frame.payload);
            }

            // Bufferbloat detected: sojourn time is above target
            if self.first_above_time.is_none() {
                self.first_above_time = Some(now + self.interval);
            } else if now >= self.first_above_time.unwrap() {
                if !self.is_dropping {
                    self.is_dropping = true;
                    self.count = 1;
                    self.drop_next = now + self.compute_drop_interval(self.count);
                    // Drop this packet, continue loop to inspect next packet
                    continue;
                } else if now >= self.drop_next {
                    self.count = self.count.saturating_add(1);
                    self.drop_next = now + self.compute_drop_interval(self.count);
                    // Drop packet
                    continue;
                }
            }

            return Some(frame.payload);
        }

        self.is_dropping = false;
        None
    }

    fn compute_drop_interval(&self, count: u32) -> Duration {
        let factor = (count as f64).sqrt();
        Duration::from_secs_f64(self.interval.as_secs_f64() / factor)
    }
}
```

