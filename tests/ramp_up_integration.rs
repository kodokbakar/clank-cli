use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clank_cli::engine::{Engine, EngineConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
struct RequestEvent {
    elapsed: Duration,
    active_at_start: usize,
}

struct TestServer {
    url: String,
    events: Arc<Mutex<Vec<RequestEvent>>>,
    handle: JoinHandle<()>,
}

impl TestServer {
    fn events(&self) -> Vec<RequestEvent> {
        self.events
            .lock()
            .expect("request events mutex should not be poisoned")
            .clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn start_timing_server(response_delay: Duration) -> Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind timing test server")?;

    let address = listener
        .local_addr()
        .context("failed to read timing test server address")?;

    let events = Arc::new(Mutex::new(Vec::new()));
    let active_requests = Arc::new(AtomicUsize::new(0));
    let started_at = Instant::now();

    let events_for_task = Arc::clone(&events);
    let active_requests_for_task = Arc::clone(&active_requests);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _peer)) = listener.accept().await else {
                break;
            };

            let events = Arc::clone(&events_for_task);
            let active_requests = Arc::clone(&active_requests_for_task);

            tokio::spawn(async move {
                let active_at_start = active_requests.fetch_add(1, Ordering::SeqCst) + 1;

                {
                    let mut events = events
                        .lock()
                        .expect("request events mutex should not be poisoned");

                    events.push(RequestEvent {
                        elapsed: started_at.elapsed(),
                        active_at_start,
                    });
                }

                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;

                tokio::time::sleep(response_delay).await;

                let response =
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

                let _ = socket.write_all(response).await;
                let _ = socket.shutdown().await;

                active_requests.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });

    Ok(TestServer {
        url: format!("http://{address}"),
        events,
        handle,
    })
}

fn engine_config(url: String, concurrency: usize) -> EngineConfig {
    EngineConfig {
        url,
        method: "GET".to_string(),
        body: None,
        headers: Vec::new(),
        concurrency,
        timeout: Duration::from_secs(2),
        insecure: false,
        rate_limit: None,
        rate_limiter: None,
        ramp_up: None,
        ramp_up_step: 1,
        keep_alive: true,
    }
}

fn arrivals_before(events: &[RequestEvent], limit: Duration) -> usize {
    events.iter().filter(|event| event.elapsed <= limit).count()
}

fn max_active(events: &[RequestEvent]) -> usize {
    events
        .iter()
        .map(|event| event.active_at_start)
        .max()
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ramp_up_gradually_increases_workers_instead_of_bursting() -> Result<()> {
    let server = start_timing_server(Duration::from_millis(700)).await?;

    let mut config = engine_config(server.url.clone(), 10);
    config.ramp_up = Some(Duration::from_millis(500));
    config.ramp_up_step = 1;

    let engine = Engine::new(config)?;
    let snapshot = engine.run_for_duration(Duration::from_millis(650)).await?;

    assert_eq!(snapshot.total_errors, 0);

    let events = server.events();

    assert!(
        !events.is_empty(),
        "expected timing server to receive requests"
    );

    let early_arrivals = arrivals_before(&events, Duration::from_millis(100));

    assert!(
        early_arrivals <= 3,
        "ramp-up should not start all workers immediately; got {early_arrivals} arrivals within first 100ms; events: {events:?}"
    );

    assert_eq!(
        max_active(&events),
        10,
        "ramp-up should eventually reach target concurrency; events: {events:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ramp_up_step_five_reaches_twenty_workers_in_four_steps() -> Result<()> {
    let server = start_timing_server(Duration::from_millis(700)).await?;

    let mut config = engine_config(server.url.clone(), 20);
    config.ramp_up = Some(Duration::from_millis(400));
    config.ramp_up_step = 5;

    let engine = Engine::new(config)?;
    let snapshot = engine.run_for_duration(Duration::from_millis(550)).await?;

    assert_eq!(snapshot.total_errors, 0);

    let events = server.events();

    let first_window = arrivals_before(&events, Duration::from_millis(75));
    let second_window = arrivals_before(&events, Duration::from_millis(175));
    let third_window = arrivals_before(&events, Duration::from_millis(275));

    assert!(
        (3..=7).contains(&first_window),
        "expected about 5 initial workers, got {first_window}; events: {events:?}"
    );

    assert!(
        (8..=12).contains(&second_window),
        "expected about 10 workers after second step, got {second_window}; events: {events:?}"
    );

    assert!(
        (13..=17).contains(&third_window),
        "expected about 15 workers after third step, got {third_window}; events: {events:?}"
    );

    assert_eq!(
        max_active(&events),
        20,
        "expected final ramp-up state to reach 20 active workers; events: {events:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_ramp_up_remains_backward_compatible_and_starts_workers_immediately() -> Result<()> {
    let server = start_timing_server(Duration::from_millis(500)).await?;

    let config = engine_config(server.url.clone(), 10);

    let engine = Engine::new(config)?;
    let snapshot = engine.run_for_duration(Duration::from_millis(150)).await?;

    assert_eq!(snapshot.total_errors, 0);

    let events = server.events();
    let early_arrivals = arrivals_before(&events, Duration::from_millis(100));

    assert!(
        early_arrivals >= 8,
        "without ramp-up, workers should start immediately; got {early_arrivals} arrivals within first 100ms; events: {events:?}"
    );

    assert_eq!(
        max_active(&events),
        10,
        "without ramp-up, target concurrency should be reached immediately; events: {events:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ramp_up_with_short_test_duration_still_completes_without_errors() -> Result<()> {
    let server = start_timing_server(Duration::from_millis(200)).await?;

    let mut config = engine_config(server.url.clone(), 10);
    config.ramp_up = Some(Duration::from_secs(1));
    config.ramp_up_step = 1;

    let engine = Engine::new(config)?;
    let snapshot = engine.run_for_duration(Duration::from_millis(300)).await?;

    assert_eq!(snapshot.total_errors, 0);
    assert!(
        snapshot.total_requests > 0,
        "short duration ramp-up should still send some requests"
    );

    let events = server.events();
    let early_arrivals = arrivals_before(&events, Duration::from_millis(100));

    assert!(
        early_arrivals <= 3,
        "short duration ramp-up should still avoid an immediate burst; got {early_arrivals}; events: {events:?}"
    );

    Ok(())
}
