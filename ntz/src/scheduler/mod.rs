//! The durable scheduler: a reconciliation loop that turns due `schedule`
//! triggers into `runs`. The database — `triggers.next_run_at` and the
//! `runs` table's `UNIQUE(container_id, scheduled_for)` — is the source of
//! truth, not an in-memory timer, so a restart resumes exactly where it left
//! off instead of losing whatever was scheduled while the process was down.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use croner::Cron;
use tokio::sync::Semaphore;

use crate::features::container;
use crate::features::run::{self, Run, RunStatus};
use crate::features::trigger::{self, CatchUp, Trigger};
use crate::AppState;

/// Captured output is truncated before storage — same reasoning as the
/// build-error cap in `container::service`.
const MAX_OUTPUT_CHARS: usize = 8 * 1024;

/// Spawns the scheduler as a background task and returns immediately. It
/// runs for the lifetime of the process, alongside the HTTP server — this
/// is the only thing that ever executes a `schedule` trigger.
pub fn spawn(state: AppState) {
    tokio::spawn(run_loop(state));
}

async fn run_loop(state: AppState) {
    let tick_interval = Duration::from_secs(state.config.scheduler_tick_seconds);
    let semaphore = Arc::new(Semaphore::new(state.config.max_concurrent_runs));

    // Tick immediately, *then* sleep: a trigger whose `next_run_at` is
    // already in the past when the process starts (it was down past that
    // slot) gets caught on this very first iteration, not up to
    // `scheduler_tick_seconds` later.
    loop {
        tick(&state, &semaphore).await;
        tokio::time::sleep(tick_interval).await;
    }
}

async fn tick(state: &AppState, semaphore: &Arc<Semaphore>) {
    let now = Utc::now();

    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || run::repo::sweep_abandoned(&db, now))
        .await
        .expect("spawn_blocking task panicked")
    {
        Ok(0) => {}
        Ok(count) => tracing::warn!(count, "swept abandoned runs back to pending"),
        Err(err) => tracing::error!(error = %err, "sweep of abandoned runs failed"),
    }

    let due_triggers = {
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || trigger::repo::find_due_schedules(&db, now)).await
    };
    let due_retries = {
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || run::repo::find_pending_retries(&db, now)).await
    };

    let mut handles = Vec::new();

    match due_triggers.expect("spawn_blocking task panicked") {
        Ok(triggers) => {
            for trig in triggers {
                let state = state.clone();
                let permit = semaphore.clone().acquire_owned().await.expect("semaphore never closed");
                handles.push(tokio::spawn(async move {
                    let _permit = permit;
                    process_trigger(&state, trig).await;
                }));
            }
        }
        Err(err) => tracing::error!(error = %err, "querying due schedule triggers failed"),
    }

    match due_retries.expect("spawn_blocking task panicked") {
        Ok(runs) => {
            for run in runs {
                let state = state.clone();
                let permit = semaphore.clone().acquire_owned().await.expect("semaphore never closed");
                handles.push(tokio::spawn(async move {
                    let _permit = permit;
                    process_pending_retry(&state, run).await;
                }));
            }
        }
        Err(err) => tracing::error!(error = %err, "querying pending retries failed"),
    }

    // Wait out this tick's fan-out before starting the next one. A run that
    // outlives the tick interval delays the next tick rather than racing
    // it — `claim_slot`/`claim_retry` are idempotent either way, but this
    // keeps behavior easy to reason about, which matters more here than
    // shaving latency off a cron scheduler.
    for handle in handles {
        let _ = handle.await;
    }
}

async fn process_trigger(state: &AppState, trig: Trigger) {
    let now = Utc::now();

    let (Some(cron_expr), Some(next_run_at)) = (&trig.cron, trig.next_run_at) else {
        tracing::error!(trigger_id = %trig.id, "due schedule trigger is missing cron or next_run_at; disabling");
        disable(state, &trig.id).await;
        return;
    };
    let cron: Cron = match cron_expr.parse() {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(trigger_id = %trig.id, cron = %cron_expr, error = %err, "trigger's cron expression no longer parses; disabling");
            disable(state, &trig.id).await;
            return;
        }
    };

    let skip_tolerance = TimeDelta::seconds(state.config.scheduler_tick_seconds as i64 * 3);
    let slots = slots_to_run(
        &cron,
        trig.catch_up,
        next_run_at,
        now,
        skip_tolerance,
        state.config.max_backfill_slots,
    );
    if trig.catch_up == CatchUp::Backfill && slots.len() == state.config.max_backfill_slots {
        tracing::warn!(
            trigger_id = %trig.id,
            capped = slots.len(),
            "backfill capped for this tick; remaining missed slots continue next tick"
        );
    }

    // Advance regardless of whether execution below actually happens (e.g.
    // the container isn't built yet) — otherwise a trigger that can't run
    // right now would be "due" again on every single tick forever.
    let new_next_run_at = slots
        .last()
        .and_then(|last| cron.find_next_occurrence(last, false).ok())
        .or_else(|| cron.find_next_occurrence(&now, false).ok());
    let Some(new_next_run_at) = new_next_run_at else {
        tracing::error!(trigger_id = %trig.id, "could not compute this trigger's next occurrence; disabling");
        disable(state, &trig.id).await;
        return;
    };

    let db = state.db.clone();
    let container_id = trig.container_id.clone();
    let container = tokio::task::spawn_blocking(move || container::repo::find(&db, &container_id))
        .await
        .expect("spawn_blocking task panicked");

    match container {
        Ok(Some(container)) if container.wasm_path.is_some() => {
            for scheduled_for in slots {
                let new_run_id = uuid::Uuid::now_v7().to_string();
                let db = state.db.clone();
                let container_id = trig.container_id.clone();
                let trigger_id = trig.id.clone();
                let version = container.version;
                let lease_minutes = state.config.run_lease_minutes;
                let claim = tokio::task::spawn_blocking(move || {
                    run::repo::claim_slot(&db, &new_run_id, &container_id, version, &trigger_id, scheduled_for, lease_minutes)
                })
                .await
                .expect("spawn_blocking task panicked");

                match claim {
                    Ok(run::repo::Claim::New(run) | run::repo::Claim::Resumed(run)) => {
                        execute_and_resolve(state, run).await;
                    }
                    Ok(run::repo::Claim::AlreadyClaimed) => {}
                    Err(err) => {
                        tracing::error!(trigger_id = %trig.id, error = %err, "claiming a scheduled slot failed")
                    }
                }
            }
        }
        Ok(Some(_)) => {
            tracing::warn!(trigger_id = %trig.id, container_id = %trig.container_id, "scheduled trigger's container has never been built; skipping this tick's slot(s)");
        }
        Ok(None) => {
            tracing::error!(trigger_id = %trig.id, container_id = %trig.container_id, "scheduled trigger points at a container that no longer exists; disabling");
            disable(state, &trig.id).await;
            return;
        }
        Err(err) => {
            tracing::error!(trigger_id = %trig.id, error = %err, "failed to look up container for scheduled trigger");
        }
    }

    let db = state.db.clone();
    let trigger_id = trig.id.clone();
    if let Err(err) = tokio::task::spawn_blocking(move || {
        trigger::repo::update_after_run(&db, &trigger_id, now, new_next_run_at)
    })
    .await
    .expect("spawn_blocking task panicked")
    {
        tracing::error!(trigger_id = %trig.id, error = %err, "failed to advance trigger's next_run_at");
    }
}

async fn process_pending_retry(state: &AppState, run: Run) {
    let db = state.db.clone();
    let run_id = run.id.clone();
    let lease_minutes = state.config.run_lease_minutes;
    let claimed = tokio::task::spawn_blocking(move || run::repo::claim_retry(&db, &run_id, lease_minutes))
        .await
        .expect("spawn_blocking task panicked");

    match claimed {
        Ok(Some(run)) => execute_and_resolve(state, run).await,
        Ok(None) => {} // someone else already claimed this retry
        Err(err) => tracing::error!(run_id = %run.id, error = %err, "claiming a pending retry failed"),
    }
}

/// Executes the run a claim produced, then resolves it: success or
/// exhausted retries become a terminal status; a failure with retries left
/// goes back to `pending` with a backoff delay. Shared by fresh/backfilled
/// slots and by retries — both end up here once they hold the lease.
async fn execute_and_resolve(state: &AppState, run: Run) {
    let outcome = container::service::run_once(state, &run.container_id, &[]).await;
    let finished_at = Utc::now();

    let (stdout, stderr, error, success, max_retries, backoff_seconds) = match outcome {
        Ok((container, o)) => (
            o.stdout,
            o.stderr,
            None,
            o.success,
            container.max_retries,
            container.retry_backoff_seconds,
        ),
        // A host-level failure (container deleted, never built, wasmtime
        // trap) isn't the kind of transient fault retries are for — treat
        // it as immediately terminal regardless of the container's policy.
        Err(err) => (String::new(), String::new(), Some(err.to_string()), false, 0, 0),
    };
    let stdout = truncate(&stdout);
    let stderr = truncate(&stderr);

    let run_id = run.id.clone();
    let db = state.db.clone();
    let result = if success {
        tokio::task::spawn_blocking(move || {
            run::repo::mark_terminal(&db, &run_id, RunStatus::Success, &stdout, &stderr, None, finished_at)
        })
        .await
    } else if run.attempt <= max_retries {
        let retry_at = finished_at + TimeDelta::seconds(backoff_seconds);
        tokio::task::spawn_blocking(move || {
            run::repo::mark_retry(&db, &run_id, run.attempt + 1, retry_at)
        })
        .await
    } else {
        tokio::task::spawn_blocking(move || {
            run::repo::mark_terminal(
                &db,
                &run_id,
                RunStatus::Failed,
                &stdout,
                &stderr,
                error.as_deref(),
                finished_at,
            )
        })
        .await
    }
    .expect("spawn_blocking task panicked");

    if let Err(err) = result {
        tracing::error!(run_id = %run.id, error = %err, "failed to persist run outcome");
    }
}

/// Which cron slots a due trigger should actually execute this tick, per
/// its `catch_up` policy. Pure function — the interesting part of the
/// catch-up design, deliberately kept separate from I/O so it's easy to
/// reason about (and test) on its own.
fn slots_to_run(
    cron: &Cron,
    catch_up: CatchUp,
    next_run_at: DateTime<Utc>,
    now: DateTime<Utc>,
    skip_tolerance: TimeDelta,
    max_backfill: usize,
) -> Vec<DateTime<Utc>> {
    match catch_up {
        // `next_after(now)` (computed by the caller once this slot is
        // processed) naturally jumps past whatever was missed — that *is*
        // the coalesce behavior, no explicit skipping needed here.
        CatchUp::Coalesce => vec![next_run_at],
        CatchUp::Skip => {
            if now - next_run_at > skip_tolerance {
                Vec::new()
            } else {
                vec![next_run_at]
            }
        }
        CatchUp::Backfill => {
            let mut slots = Vec::new();
            let mut t = next_run_at;
            while t <= now && slots.len() < max_backfill {
                slots.push(t);
                t = match cron.find_next_occurrence(&t, false) {
                    Ok(next) => next,
                    Err(_) => break,
                };
            }
            slots
        }
    }
}

async fn disable(state: &AppState, trigger_id: &str) {
    let db = state.db.clone();
    let trigger_id = trigger_id.to_owned();
    if let Err(err) = tokio::task::spawn_blocking(move || trigger::repo::disable(&db, &trigger_id))
        .await
        .expect("spawn_blocking task panicked")
    {
        tracing::error!(error = %err, "failed to disable a broken schedule trigger");
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(MAX_OUTPUT_CHARS).collect()
}
