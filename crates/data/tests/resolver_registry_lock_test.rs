use arclain_data::{
    DataRequest, DataService, DataSource, DataSourceResolver, DataStatus, ResolveError,
};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

const CHANNEL_TIMEOUT: Duration = Duration::from_secs(5);

struct CallbackGate {
    entered: Mutex<Option<Sender<()>>>,
    release: Mutex<Receiver<()>>,
}

impl CallbackGate {
    fn new() -> (Self, Receiver<()>, Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        (
            Self {
                entered: Mutex::new(Some(entered_tx)),
                release: Mutex::new(release_rx),
            },
            entered_rx,
            release_tx,
        )
    }

    fn wait(&self) {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
        }
        self.release.lock().unwrap().recv().unwrap();
    }
}

struct BlockingResolveResolver(CallbackGate);

impl BlockingResolveResolver {
    fn new() -> (Self, Receiver<()>, Sender<()>) {
        let (gate, entered, release) = CallbackGate::new();
        (Self(gate), entered, release)
    }
}

impl DataSourceResolver for BlockingResolveResolver {
    fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        self.0.wait();
        Err(ResolveError::NotFound)
    }
}

struct BlockingCacheStoreResolver(CallbackGate);

impl BlockingCacheStoreResolver {
    fn new() -> (Self, Receiver<()>, Sender<()>) {
        let (gate, entered, release) = CallbackGate::new();
        (Self(gate), entered, release)
    }
}

impl DataSourceResolver for BlockingCacheStoreResolver {
    fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        Err(ResolveError::NotFound)
    }

    fn try_store(
        &self,
        _key: &str,
        _data: &[u8],
        _request: &DataRequest,
    ) -> Result<(), ResolveError> {
        self.0.wait();
        Ok(())
    }
}

struct SuccessfulNetworkResolver;

impl DataSourceResolver for SuccessfulNetworkResolver {
    fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        Ok(b"network payload".to_vec())
    }
}

struct BlockingHasResolver(CallbackGate);

impl BlockingHasResolver {
    fn new() -> (Self, Receiver<()>, Sender<()>) {
        let (gate, entered, release) = CallbackGate::new();
        (Self(gate), entered, release)
    }
}

impl DataSourceResolver for BlockingHasResolver {
    fn try_resolve(&self, _key: &str, _request: &DataRequest) -> Result<Vec<u8>, ResolveError> {
        Err(ResolveError::NotFound)
    }

    fn has(&self, _key: &str, _request: &DataRequest) -> bool {
        self.0.wait();
        false
    }

    fn has_with_limit(&self, key: &str, request: &DataRequest, _limit: usize) -> bool {
        self.has(key, request)
    }
}

fn registration_worker(
    service: DataService,
) -> (Receiver<()>, Receiver<()>, thread::JoinHandle<()>) {
    let (attempted_tx, attempted_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        attempted_tx.send(()).unwrap();
        service.register_resolver(DataSource::Memory, Arc::new(SuccessfulNetworkResolver));
        completed_tx.send(()).unwrap();
    });

    (attempted_rx, completed_rx, worker)
}

fn assert_registration_completes_while_blocked<T>(
    service: DataService,
    entered_rx: Receiver<()>,
    release_tx: Sender<()>,
    operation_worker: thread::JoinHandle<T>,
    callback_name: &str,
) -> T {
    let callback_entered = entered_rx.recv_timeout(CHANNEL_TIMEOUT).is_ok();
    let (attempted_rx, completed_rx, registration_worker) = registration_worker(service);
    let registration_attempted = attempted_rx.recv_timeout(CHANNEL_TIMEOUT).is_ok();
    let registration_completed_while_blocked = completed_rx.recv_timeout(CHANNEL_TIMEOUT).is_ok();

    release_tx.send(()).ok();
    let operation_result = operation_worker.join().expect("operation worker panicked");
    registration_worker
        .join()
        .expect("registration worker panicked");

    assert!(callback_entered, "{callback_name} was never entered");
    assert!(registration_attempted, "registration worker never started");
    assert!(
        registration_completed_while_blocked,
        "{callback_name} retained the registry read lock"
    );

    operation_result
}

#[test]
fn registration_completes_while_resolution_callback_is_blocked() {
    let service = DataService::new();
    let (resolver, entered_rx, release_tx) = BlockingResolveResolver::new();
    service.register_resolver(DataSource::Network, Arc::new(resolver));

    let resolve_service = service.clone();
    let resolve_worker = thread::spawn(move || {
        resolve_service.resolve(&DataRequest::network_only("key", "https://example.invalid"))
    });

    let result = assert_registration_completes_while_blocked(
        service,
        entered_rx,
        release_tx,
        resolve_worker,
        "resolver callback",
    );
    assert_eq!(result.status, DataStatus::Failed);
}

#[test]
fn registration_completes_while_recursive_cache_store_is_blocked() {
    let service = DataService::new();
    let (cache, entered_rx, release_tx) = BlockingCacheStoreResolver::new();
    service.register_resolver(DataSource::ContentCache, Arc::new(cache));
    service.register_resolver(DataSource::Network, Arc::new(SuccessfulNetworkResolver));

    let resolve_service = service.clone();
    let resolve_worker = thread::spawn(move || {
        resolve_service.resolve(&DataRequest::cache_first(
            "payload",
            "https://example.invalid",
        ))
    });

    let result = assert_registration_completes_while_blocked(
        service,
        entered_rx,
        release_tx,
        resolve_worker,
        "recursive cache store callback",
    );
    assert_eq!(result.status, DataStatus::Ready);
    assert_eq!(result.data.as_deref(), Some(b"network payload".as_slice()));
}

#[test]
fn registration_completes_while_save_callback_is_blocked() {
    let service = DataService::new();
    let (resolver, entered_rx, release_tx) = BlockingCacheStoreResolver::new();
    service.register_resolver(DataSource::ContentCache, Arc::new(resolver));

    let save_service = service.clone();
    let save_worker =
        thread::spawn(move || save_service.save_data(DataSource::ContentCache, "key", b"payload"));

    let save_result = assert_registration_completes_while_blocked(
        service,
        entered_rx,
        release_tx,
        save_worker,
        "save callback",
    );
    assert!(save_result.is_ok());
}

#[test]
fn registration_completes_while_has_callback_is_blocked() {
    let service = DataService::new();
    let (resolver, entered_rx, release_tx) = BlockingHasResolver::new();
    service.register_resolver(DataSource::MetadataStore, Arc::new(resolver));

    let has_service = service.clone();
    let has_worker = thread::spawn(move || has_service.has_data("key"));

    let has_result = assert_registration_completes_while_blocked(
        service,
        entered_rx,
        release_tx,
        has_worker,
        "has callback",
    );
    assert!(!has_result);
}
