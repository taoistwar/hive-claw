use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use http_body_util::{BodyStream, StreamBody};
use std::{
    future::{Future, IntoFuture},
    io,
    time::Duration,
};
use tokio::sync::{oneshot, watch};

pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Tokio detaches a spawned task when its JoinHandle is dropped. Request
/// handlers use this guard for child tasks that own AppState resources so
/// dropping the request future at the shutdown deadline also aborts the child.
pub(crate) struct AbortTaskOnDrop {
    abort_handle: tokio::task::AbortHandle,
}

impl AbortTaskOnDrop {
    pub(crate) fn new(abort_handle: tokio::task::AbortHandle) -> Self {
        Self { abort_handle }
    }
}

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Drained,
    Forced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownSignalSource {
    Interrupt,
    #[cfg(unix)]
    Terminate,
}

impl ShutdownSignalSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "sigint",
            #[cfg(unix)]
            Self::Terminate => "sigterm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownPhase {
    Running,
    Draining,
    Forced,
}

#[derive(Clone)]
struct ShutdownController {
    phase: watch::Sender<ShutdownPhase>,
}

impl ShutdownController {
    fn new() -> Self {
        let (phase, _) = watch::channel(ShutdownPhase::Running);
        Self { phase }
    }

    fn phase(&self) -> ShutdownPhase {
        *self.phase.borrow()
    }

    fn subscribe(&self) -> watch::Receiver<ShutdownPhase> {
        self.phase.subscribe()
    }

    fn begin_draining(&self) {
        if self.phase() == ShutdownPhase::Running {
            self.phase.send_replace(ShutdownPhase::Draining);
        }
    }

    fn force(&self) {
        self.phase.send_replace(ShutdownPhase::Forced);
    }
}

async fn wait_for_force(receiver: &mut watch::Receiver<ShutdownPhase>) {
    loop {
        let forced = *receiver.borrow_and_update() == ShutdownPhase::Forced;
        if forced {
            return;
        }
        if receiver.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn shutdown_response() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "server is shutting down").into_response()
}

fn cancel_response_body_on_force(
    response: Response,
    receiver: watch::Receiver<ShutdownPhase>,
) -> Response {
    let (parts, body) = response.into_parts();
    let stream = BodyStream::new(body);
    let stream = futures::stream::unfold(
        (Box::pin(stream), receiver),
        |(mut stream, mut receiver)| async move {
            tokio::select! {
                item = stream.next() => item.map(|item| (item, (stream, receiver))),
                _ = wait_for_force(&mut receiver) => None,
            }
        },
    );
    Response::from_parts(parts, Body::new(StreamBody::new(stream)))
}

async fn shutdown_request_guard(
    State(controller): State<ShutdownController>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if controller.phase() != ShutdownPhase::Running {
        return shutdown_response();
    }

    let mut force = controller.subscribe();
    let response = tokio::select! {
        response = next.run(request) => response,
        _ = wait_for_force(&mut force) => return shutdown_response(),
    };
    cancel_response_body_on_force(response, controller.subscribe())
}

#[cfg(unix)]
async fn wait_for_first_signal<I, T>(interrupt: I, terminate: T) -> ShutdownSignalSource
where
    I: Future<Output = ()>,
    T: Future<Output = ()>,
{
    tokio::select! {
        _ = interrupt => ShutdownSignalSource::Interrupt,
        _ = terminate => ShutdownSignalSource::Terminate,
    }
}

async fn interrupt_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        tracing::error!(
            error_kind = "shutdown_interrupt_handler_failed",
            "shutdown interrupt handler failed"
        );
    }
}

#[cfg(unix)]
async fn terminate_signal() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut signal) => {
            let _ = signal.recv().await;
        }
        Err(_) => {
            tracing::error!(
                error_kind = "shutdown_terminate_handler_failed",
                "shutdown terminate handler failed"
            );
            std::future::pending::<()>().await;
        }
    }
}

/// Wait for the platform shutdown signal without logging OS error details.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    let source = wait_for_first_signal(interrupt_signal(), terminate_signal()).await;

    #[cfg(not(unix))]
    let source = {
        interrupt_signal().await;
        ShutdownSignalSource::Interrupt
    };

    tracing::info!(
        event = "shutdown_signal_received",
        signal = source.as_str(),
        "shutdown signal received"
    );
}

/// Serve until SIGINT/SIGTERM and give active requests the fixed production
/// drain budget before cancelling their handler and response-body futures.
pub async fn serve_with_graceful_shutdown<F>(
    listener: tokio::net::TcpListener,
    app: Router,
    signal: F,
) -> io::Result<ShutdownOutcome>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_drain_timeout(listener, app, signal, DEFAULT_DRAIN_TIMEOUT).await
}

async fn serve_with_drain_timeout<F>(
    listener: tokio::net::TcpListener,
    app: Router,
    signal: F,
    drain_timeout: Duration,
) -> io::Result<ShutdownOutcome>
where
    F: Future<Output = ()> + Send + 'static,
{
    let controller = ShutdownController::new();
    let guarded_app = app.layer(middleware::from_fn_with_state(
        controller.clone(),
        shutdown_request_guard,
    ));
    let (signal_seen_tx, mut signal_seen_rx) = oneshot::channel();
    let signal_controller = controller.clone();
    let graceful_signal = async move {
        signal.await;
        signal_controller.begin_draining();
        let _ = signal_seen_tx.send(());
    };
    let server = axum::serve(listener, guarded_app)
        .with_graceful_shutdown(graceful_signal)
        .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = server.as_mut() => {
            result?;
            Ok(ShutdownOutcome::Drained)
        }
        signal_result = &mut signal_seen_rx => {
            if signal_result.is_err() {
                return server.as_mut().await.map(|()| ShutdownOutcome::Drained);
            }
            match tokio::time::timeout(drain_timeout, server.as_mut()).await {
                Ok(result) => {
                    result?;
                    Ok(ShutdownOutcome::Drained)
                }
                Err(_) => {
                    controller.force();
                    server.as_mut().await?;
                    Ok(ShutdownOutcome::Forced)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AbortTaskOnDrop, DEFAULT_DRAIN_TIMEOUT, ShutdownOutcome, serve_with_drain_timeout,
    };
    #[cfg(unix)]
    use super::{ShutdownSignalSource, wait_for_first_signal};
    use axum::{
        Router, body::Body, extract::State, http::StatusCode, response::Response, routing::get,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::{Notify, oneshot};

    struct ResourceProbe(Arc<AtomicBool>);

    impl Drop for ResourceProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    struct ActiveRequestProbe(Arc<AtomicBool>);

    impl Drop for ActiveRequestProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    struct SlowState {
        started: Arc<Notify>,
        release: Arc<Notify>,
        active_request_dropped: Arc<AtomicBool>,
        _resource: Arc<ResourceProbe>,
    }

    async fn releasable_handler(State(state): State<SlowState>) -> StatusCode {
        let _active = ActiveRequestProbe(Arc::clone(&state.active_request_dropped));
        state.started.notify_one();
        state.release.notified().await;
        StatusCode::OK
    }

    async fn pending_handler(State(state): State<SlowState>) -> StatusCode {
        let _active = ActiveRequestProbe(Arc::clone(&state.active_request_dropped));
        state.started.notify_one();
        std::future::pending::<()>().await;
        StatusCode::OK
    }

    async fn pending_body_handler(State(state): State<SlowState>) -> Response {
        state.started.notify_one();
        let active = ActiveRequestProbe(Arc::clone(&state.active_request_dropped));
        let stream = futures::stream::unfold(active, |active| async move {
            std::future::pending::<()>().await;
            Some((Ok::<_, std::io::Error>(bytes::Bytes::new()), active))
        });
        Response::new(Body::from_stream(stream))
    }

    fn slow_state() -> (SlowState, Arc<AtomicBool>) {
        let resource_dropped = Arc::new(AtomicBool::new(false));
        (
            SlowState {
                started: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
                active_request_dropped: Arc::new(AtomicBool::new(false)),
                _resource: Arc::new(ResourceProbe(Arc::clone(&resource_dropped))),
            },
            resource_dropped,
        )
    }

    async fn wait_until_listener_stops_accepting(address: std::net::SocketAddr) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match tokio::net::TcpStream::connect(address).await {
                    Ok(stream) => {
                        drop(stream);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Err(_) => return,
                }
            }
        })
        .await
        .expect("shutdown must stop accepting new TCP connections");
    }

    #[test]
    fn production_drain_timeout_is_fixed_at_thirty_seconds() {
        assert_eq!(DEFAULT_DRAIN_TIMEOUT, Duration::from_secs(30));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn first_shutdown_signal_accepts_interrupt_or_terminate() {
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let interrupt = wait_for_first_signal(
            async move {
                let _ = interrupt_rx.await;
            },
            std::future::pending(),
        );
        interrupt_tx.send(()).unwrap();
        assert_eq!(interrupt.await, ShutdownSignalSource::Interrupt);

        let (terminate_tx, terminate_rx) = oneshot::channel();
        let terminate = wait_for_first_signal(std::future::pending(), async move {
            let _ = terminate_rx.await;
        });
        terminate_tx.send(()).unwrap();
        assert_eq!(terminate.await, ShutdownSignalSource::Terminate);
    }

    #[tokio::test]
    async fn abort_task_guard_cancels_detached_request_child_on_drop() {
        struct TaskProbe(Arc<AtomicBool>);

        impl Drop for TaskProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let task_dropped = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = oneshot::channel();
        let handle = tokio::spawn({
            let task_dropped = Arc::clone(&task_dropped);
            async move {
                let _probe = TaskProbe(task_dropped);
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            }
        });
        started_rx.await.unwrap();
        let guard = AbortTaskOnDrop::new(handle.abort_handle());

        drop(guard);
        let error = handle.await.expect_err("guard must abort the child task");

        assert!(error.is_cancelled());
        assert!(task_dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn shutdown_stops_accepting_then_drains_active_request_and_drops_router_state() {
        let (state, resource_dropped) = slow_state();
        let started = Arc::clone(&state.started);
        let release = Arc::clone(&state.release);
        let active_request_dropped = Arc::clone(&state.active_request_dropped);
        let app = Router::new()
            .route("/", get(releasable_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (signal_tx, signal_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_drain_timeout(
            listener,
            app,
            async move {
                let _ = signal_rx.await;
            },
            Duration::from_secs(1),
        ));

        let request = tokio::spawn(async move {
            reqwest::Client::new()
                .get(format!("http://{address}/"))
                .send()
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("handler must start");

        signal_tx.send(()).unwrap();
        wait_until_listener_stops_accepting(address).await;
        assert!(
            !server.is_finished(),
            "server must keep draining the active request"
        );

        release.notify_one();
        let response = request.await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(server.await.unwrap().unwrap(), ShutdownOutcome::Drained);
        assert!(active_request_dropped.load(Ordering::SeqCst));
        assert!(
            resource_dropped.load(Ordering::SeqCst),
            "returning from the server must drop Router/AppState resources"
        );
    }

    #[tokio::test]
    async fn shutdown_deadline_drops_active_future_and_router_state() {
        let (state, resource_dropped) = slow_state();
        let started = Arc::clone(&state.started);
        let active_request_dropped = Arc::clone(&state.active_request_dropped);
        let app = Router::new()
            .route("/", get(pending_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (signal_tx, signal_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_drain_timeout(
            listener,
            app,
            async move {
                let _ = signal_rx.await;
            },
            Duration::from_millis(20),
        ));

        let request = tokio::spawn(async move {
            reqwest::Client::new()
                .get(format!("http://{address}/"))
                .send()
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("handler must start");

        signal_tx.send(()).unwrap();
        wait_until_listener_stops_accepting(address).await;
        assert_eq!(server.await.unwrap().unwrap(), ShutdownOutcome::Forced);
        assert!(
            active_request_dropped.load(Ordering::SeqCst),
            "deadline must drop the active handler future"
        );
        assert!(
            resource_dropped.load(Ordering::SeqCst),
            "forced shutdown must release Router/AppState resources"
        );
        let _ = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .expect("forced request task must finish");
    }

    #[tokio::test]
    async fn shutdown_deadline_drops_streaming_response_body_and_router_state() {
        let (state, resource_dropped) = slow_state();
        let started = Arc::clone(&state.started);
        let active_request_dropped = Arc::clone(&state.active_request_dropped);
        let app = Router::new()
            .route("/", get(pending_body_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (signal_tx, signal_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_drain_timeout(
            listener,
            app,
            async move {
                let _ = signal_rx.await;
            },
            Duration::from_millis(20),
        ));

        let request = tokio::spawn(async move {
            reqwest::Client::new()
                .get(format!("http://{address}/"))
                .send()
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("streaming handler must start");
        let response = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .expect("streaming response headers must arrive")
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        signal_tx.send(()).unwrap();
        wait_until_listener_stops_accepting(address).await;
        assert_eq!(server.await.unwrap().unwrap(), ShutdownOutcome::Forced);
        assert!(
            active_request_dropped.load(Ordering::SeqCst),
            "deadline must drop an active streaming response body"
        );
        assert!(
            resource_dropped.load(Ordering::SeqCst),
            "forced streaming shutdown must release Router/AppState resources"
        );
        drop(response);
    }
}
