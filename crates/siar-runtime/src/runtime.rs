//! §51-§54, §204: the runtime core — the *same* command/query/event API
//! whether called in-process, over IPC, or (later) FFI (§54).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use siar_power::{PowerPolicyConfig, PowerScheduler, RawPowerReading, SchedulerConfig};

use crate::dedup::{Begin, CommandId, CommandLedger, CommandOutcome, Principal};
use crate::error::RuntimeError;
use crate::events::{EventBus, EventPriority, Topic};
use crate::health::HealthRegistry;
use crate::lifecycle::{Lifecycle, RuntimeState};
use crate::protocol::{Command, Query, Reply, StatusSnapshot, PROTOCOL_VERSION};
use crate::supervisor::Supervisor;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Reserved topics (application topics start at 0x100).
pub mod topics {
    use crate::events::Topic;
    pub const LIFECYCLE: Topic = Topic(1);
    pub const POWER: Topic = Topic(2);
    pub const HEALTH: Topic = Topic(3);
}

/// What the embedding application plugs in (messaging, relay, files…).
pub trait AppService: Send + Sync + 'static {
    fn command<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>>;
    fn query<'a>(
        &'a self,
        name: &'a str,
        data: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>>;
}

/// An `AppService` that supports nothing; for tests and bare nodes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoApp;

impl AppService for NoApp {
    fn command<'a>(
        &'a self,
        _: &'a str,
        _: Vec<u8>,
    ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(async { Err(RuntimeError::Unsupported) })
    }
    fn query<'a>(&'a self, _: &'a str, _: Vec<u8>) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
        Box::pin(async { Err(RuntimeError::Unsupported) })
    }
}

/// §52.
pub struct RuntimeBuilder {
    app: Arc<dyn AppService>,
    power: PowerPolicyConfig,
    sched: SchedulerConfig,
    ledger_cap: usize,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            app: Arc::new(NoApp),
            power: PowerPolicyConfig::default(),
            sched: SchedulerConfig::default(),
            ledger_cap: 4096,
        }
    }
}

impl RuntimeBuilder {
    #[must_use]
    pub fn app(mut self, app: impl AppService) -> Self {
        self.app = Arc::new(app);
        self
    }
    #[must_use]
    pub fn power_policy(mut self, c: PowerPolicyConfig) -> Self {
        self.power = c;
        self
    }
    #[must_use]
    pub fn scheduler(mut self, c: SchedulerConfig) -> Self {
        self.sched = c;
        self
    }
    #[must_use]
    pub fn ledger_capacity(mut self, n: usize) -> Self {
        self.ledger_cap = n;
        self
    }

    #[must_use]
    pub fn build(self) -> Runtime {
        let lifecycle = Lifecycle::new();
        let health = HealthRegistry::new();
        let instance_id = uuid::Uuid::new_v4().as_u128();
        Runtime {
            supervisor: Supervisor::new(lifecycle.clone(), health.clone()),
            lifecycle,
            health,
            bus: EventBus::new(instance_id),
            ledger: Arc::new(Mutex::new(CommandLedger::new(self.ledger_cap))),
            power: Arc::new(Mutex::new(PowerScheduler::new(self.power, self.sched))),
            app: self.app,
            started: Instant::now(),
        }
    }
}

/// §53. Cheap to clone; all clones share one runtime.
#[derive(Clone)]
pub struct Runtime {
    pub lifecycle: Lifecycle,
    pub health: HealthRegistry,
    pub supervisor: Supervisor,
    pub bus: EventBus,
    ledger: Arc<Mutex<CommandLedger>>,
    power: Arc<Mutex<PowerScheduler>>,
    app: Arc<dyn AppService>,
    started: Instant,
}

impl Runtime {
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    /// §33-§34: recovery first, then services, then — and only then — the
    /// network barrier opens (`Ready`). A recovery error leaves the runtime
    /// `ReadOnly` (§28/§111): readable, never writing on top of unrecovered state.
    pub async fn start<R, S, E1, E2>(
        &self,
        recover: R,
        start_services: S,
    ) -> Result<(), RuntimeError>
    where
        R: Future<Output = Result<(), E1>>,
        S: Future<Output = Result<(), E2>>,
        E1: std::fmt::Display,
        E2: std::fmt::Display,
    {
        let lc = &self.lifecycle;
        lc.transition(RuntimeState::Recovering)
            .map_err(|_| RuntimeError::Busy)?;
        self.announce_state();
        if let Err(e) = recover.await {
            tracing::error!(error = %e, "startup recovery failed; entering read-only");
            let _ = lc.transition(RuntimeState::ReadOnly);
            self.announce_state();
            return Err(RuntimeError::Storage);
        }
        lc.transition(RuntimeState::StartingServices)
            .map_err(|_| RuntimeError::Busy)?;
        if let Err(e) = start_services.await {
            tracing::error!(error = %e, "service start failed");
            lc.begin_shutdown();
            self.announce_state();
            return Err(RuntimeError::Network);
        }
        let _ = lc.transition(RuntimeState::Ready);
        crate::supervisor::sync_lifecycle(lc, &self.health);
        self.announce_state();
        Ok(())
    }

    fn announce_state(&self) {
        let payload = postcard::to_allocvec(&self.lifecycle.state()).unwrap_or_default();
        self.bus
            .publish(topics::LIFECYCLE, EventPriority::Critical, None, payload);
    }

    #[must_use]
    pub fn state(&self) -> RuntimeState {
        self.lifecycle.state()
    }

    /// §35: stop accepting work, drain workers (up to `grace`), mark stopped.
    pub async fn shutdown(&self, grace: Duration) -> usize {
        self.lifecycle.begin_shutdown();
        self.announce_state();
        let aborted = self.supervisor.drain(grace).await;
        let _ = self.lifecycle.transition(RuntimeState::Stopped);
        aborted
    }

    /// Access to the power scheduler (feed readings, submit work, poll).
    pub fn with_power<T>(&self, f: impl FnOnce(&mut PowerScheduler) -> T) -> T {
        f(&mut self
            .power
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Feed a platform reading; publishes a `POWER` event only on a
    /// meaningful transition (§84) and returns whether it did.
    pub fn observe_power(&self, raw: &RawPowerReading, now_ms: u64) -> bool {
        let t = self.with_power(|p| p.observe(raw, now_ms));
        if let Some(t) = t {
            let payload = postcard::to_allocvec(&(t.epoch, t.profile)).unwrap_or_default();
            self.bus
                .publish(topics::POWER, EventPriority::Normal, None, payload);
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn status(&self) -> StatusSnapshot {
        StatusSnapshot {
            state: self.state(),
            instance_id: self.bus.instance_id(),
            uptime_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            subscribers: u32::try_from(self.bus.subscriber_count()).unwrap_or(u32::MAX),
            protocol_version: PROTOCOL_VERSION,
        }
    }

    pub async fn query(&self, q: Query) -> Result<Reply, RuntimeError> {
        if !self.state().accepts_reads() {
            return Err(RuntimeError::NotReady);
        }
        match q {
            Query::Status => Ok(Reply::Status(self.status())),
            Query::Health => Ok(Reply::Health(self.health.report())),
            Query::Power => Ok(Reply::Power(Box::new(self.with_power(|p| p.diagnostics())))),
            Query::App { name, data } => self.app.query(&name, data).await.map(Reply::Bytes),
        }
    }

    /// Execute a command with idempotent-retry semantics (§57-§58). The
    /// caller (IPC server or in-process client) has already checked the role.
    pub async fn command(&self, who: Principal, id: CommandId, cmd: Command) -> CommandOutcome {
        match self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin(who, id)?
        {
            Begin::Done(r) => return r,
            Begin::InFlight => return Err(RuntimeError::Busy),
            Begin::New => {}
        }
        let outcome = self.execute(&cmd).await;
        let mut ledger = self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(outcome, Err(RuntimeError::Cancelled)) {
            ledger.abandon(who, id);
        } else {
            ledger.complete(who, id, outcome.clone());
        }
        outcome
    }

    async fn execute(&self, cmd: &Command) -> CommandOutcome {
        let state = self.state();
        if cmd.is_mutation() && !state.accepts_mutations() {
            return Err(match state {
                RuntimeState::ReadOnly | RuntimeState::Maintenance => RuntimeError::ReadOnly,
                _ => RuntimeError::NotReady,
            });
        }
        match cmd {
            Command::Shutdown => {
                self.lifecycle.begin_shutdown();
                self.announce_state();
                Ok(Vec::new())
            }
            Command::SetEmergency(on) => {
                let changed = self.with_power(|p| p.set_emergency(*on)).is_some();
                if changed {
                    self.bus.publish(
                        topics::POWER,
                        EventPriority::Critical,
                        None,
                        postcard::to_allocvec(on).unwrap_or_default(),
                    );
                }
                Ok(Vec::new())
            }
            Command::SetMaintenance(on) => self.toggle(*on, RuntimeState::Maintenance),
            Command::SetReadOnly(on) => self.toggle(*on, RuntimeState::ReadOnly),
            Command::App { name, data } => self.app.command(name, data.clone()).await,
        }
    }

    fn toggle(&self, on: bool, mode: RuntimeState) -> CommandOutcome {
        let target = if on { mode } else { RuntimeState::Ready };
        self.lifecycle
            .transition(target)
            .map_err(|_| RuntimeError::Busy)?;
        if !on {
            crate::supervisor::sync_lifecycle(&self.lifecycle, &self.health);
        }
        self.announce_state();
        Ok(Vec::new())
    }

    /// Topic list for subscribers that want only runtime-level events.
    #[must_use]
    pub fn runtime_topics() -> Vec<Topic> {
        vec![topics::LIFECYCLE, topics::POWER, topics::HEALTH]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Counter(Arc<AtomicU32>);
    impl AppService for Counter {
        fn command<'a>(
            &'a self,
            name: &'a str,
            data: Vec<u8>,
        ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
            Box::pin(async move {
                if name == "inc" {
                    Ok(vec![(self.0.fetch_add(1, Ordering::SeqCst) + 1) as u8])
                } else {
                    let _ = data;
                    Err(RuntimeError::Unsupported)
                }
            })
        }
        fn query<'a>(
            &'a self,
            _: &'a str,
            _: Vec<u8>,
        ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
            Box::pin(async { Ok(vec![self.0.load(Ordering::SeqCst) as u8]) })
        }
    }

    async fn ready(rt: &Runtime) {
        rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retried_command_applies_once() {
        let n = Arc::new(AtomicU32::new(0));
        let rt = Runtime::builder().app(Counter(n.clone())).build();
        ready(&rt).await;
        let cmd = Command::App {
            name: "inc".into(),
            data: vec![],
        };
        let a = rt.command(1, CommandId(1), cmd.clone()).await.unwrap();
        let b = rt.command(1, CommandId(1), cmd.clone()).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(
            n.load(Ordering::SeqCst),
            1,
            "§58: the retry did not re-run the command"
        );
        rt.command(1, CommandId(2), cmd).await.unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn network_barrier_opens_only_after_recovery() {
        let rt = Runtime::builder().build();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (rt2, s2) = (rt.clone(), seen.clone());
        rt.start(
            async move {
                s2.lock().unwrap().push(rt2.state());
                Ok::<_, String>(())
            },
            async { Ok::<_, String>(()) },
        )
        .await
        .unwrap();
        assert_eq!(seen.lock().unwrap().as_slice(), &[RuntimeState::Recovering]);
        assert!(rt.state().network_open());
    }

    #[tokio::test]
    async fn failed_recovery_leaves_read_only_and_refuses_writes() {
        let rt = Runtime::builder().app(NoApp).build();
        let r = rt
            .start(async { Err::<(), _>("corrupt") }, async {
                Ok::<_, String>(())
            })
            .await;
        assert_eq!(r, Err(RuntimeError::Storage));
        assert_eq!(rt.state(), RuntimeState::ReadOnly);
        assert_eq!(
            rt.command(1, CommandId(1), Command::SetEmergency(true))
                .await,
            Err(RuntimeError::ReadOnly)
        );
        assert!(rt.query(Query::Status).await.is_ok(), "reads still work");
    }

    #[tokio::test]
    async fn maintenance_mode_round_trip() {
        let rt = Runtime::builder().build();
        ready(&rt).await;
        rt.command(1, CommandId(1), Command::SetMaintenance(true))
            .await
            .unwrap();
        assert_eq!(rt.state(), RuntimeState::Maintenance);
        assert_eq!(
            rt.command(
                1,
                CommandId(2),
                Command::App {
                    name: "x".into(),
                    data: vec![]
                }
            )
            .await,
            Err(RuntimeError::ReadOnly)
        );
        rt.command(1, CommandId(3), Command::SetMaintenance(false))
            .await
            .unwrap();
        assert_eq!(rt.state(), RuntimeState::Ready);
    }

    #[tokio::test]
    async fn emergency_command_drives_the_power_profile_and_publishes() {
        let rt = Runtime::builder().build();
        ready(&rt).await;
        let sub = rt.bus.subscribe(vec![topics::POWER], 8);
        rt.command(1, CommandId(1), Command::SetEmergency(true))
            .await
            .unwrap();
        let d = rt.query(Query::Power).await.unwrap();
        assert!(
            matches!(d, Reply::Power(p) if p.profile == siar_power::PowerPolicyProfile::Emergency)
        );
        assert!(sub.try_recv().is_some());
    }

    #[tokio::test]
    async fn power_events_are_coalesced_to_real_transitions() {
        let rt = Runtime::builder().build();
        ready(&rt).await;
        let r = RawPowerReading {
            battery_percent: Some(60),
            charging: siar_power::ChargingState::NotCharging,
            ..Default::default()
        };
        assert!(rt.observe_power(&r, 0));
        let sub = rt.bus.subscribe(vec![topics::POWER], 8);
        for p in [59, 58, 60, 61] {
            assert!(!rt.observe_power(
                &RawPowerReading {
                    battery_percent: Some(p),
                    ..r
                },
                1
            ));
        }
        assert!(sub.try_recv().is_none());
    }

    #[tokio::test]
    async fn shutdown_command_drains_and_stops() {
        let rt = Runtime::builder().build();
        ready(&rt).await;
        rt.command(1, CommandId(1), Command::Shutdown)
            .await
            .unwrap();
        assert_eq!(rt.state(), RuntimeState::ShuttingDown);
        assert_eq!(rt.shutdown(Duration::from_secs(1)).await, 0);
        assert_eq!(rt.state(), RuntimeState::Stopped);
        assert!(rt
            .command(1, CommandId(9), Command::SetEmergency(true))
            .await
            .is_err());
    }
}
