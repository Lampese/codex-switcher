//! Single-writer account store with bounded update admission.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result};
use tempfile::NamedTempFile;
use tokio::sync::{mpsc, oneshot};

use crate::types::AccountsStore;

const UPDATE_QUEUE_CAPACITY: usize = 16;

static DEFAULT_CLIENT: OnceLock<AccountStoreClient> = OnceLock::new();

type UpdateRequest = Box<dyn ErasedUpdate>;

enum WorkerRequest {
    Update(UpdateRequest),
    Shutdown,
}

/// Cheap snapshot reads plus bounded admission to the sole account-store writer.
#[derive(Clone)]
pub(crate) struct AccountStoreClient {
    requests: mpsc::Sender<WorkerRequest>,
    snapshot: Arc<RwLock<AccountsStore>>,
}

/// Guard-bound worker capability. The worker loop is unreachable without this value.
struct AccountStoreWorkerGuard {
    path: PathBuf,
    control: mpsc::Sender<WorkerRequest>,
    requests: mpsc::Receiver<WorkerRequest>,
    snapshot: Arc<RwLock<AccountsStore>>,
    current: AccountsStore,
}

/// Keeps the single writer alive and drains it during normal process shutdown.
pub(crate) struct AccountStoreRuntimeGuard {
    requests: mpsc::Sender<WorkerRequest>,
    worker: Option<JoinHandle<()>>,
}

trait ErasedUpdate: Send {
    fn execute(
        self: Box<Self>,
        current: &mut AccountsStore,
        snapshot: &RwLock<AccountsStore>,
        path: &Path,
    );
}

struct TypedUpdate<Update, Output> {
    update: Update,
    reply: oneshot::Sender<Result<Output>>,
}

impl AccountStoreClient {
    fn open(path: PathBuf) -> Result<(Self, AccountStoreWorkerGuard)> {
        let current = read_store(&path)?;
        let snapshot = Arc::new(RwLock::new(current.clone()));
        let (requests, receiver) = mpsc::channel(UPDATE_QUEUE_CAPACITY);
        Ok((
            Self {
                requests: requests.clone(),
                snapshot: Arc::clone(&snapshot),
            },
            AccountStoreWorkerGuard {
                path,
                control: requests.clone(),
                requests: receiver,
                snapshot,
                current,
            },
        ))
    }

    pub(crate) fn snapshot(&self) -> Result<AccountsStore> {
        self.snapshot
            .read()
            .map(|store| store.clone())
            .map_err(|error| anyhow::anyhow!("Account store snapshot lock is poisoned: {error}"))
    }

    pub(crate) async fn update<Output, Update>(&self, update: Update) -> Result<Output>
    where
        Output: Send + 'static,
        Update: FnOnce(&mut AccountsStore) -> Result<Output> + Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(WorkerRequest::Update(Box::new(TypedUpdate {
                update,
                reply,
            })))
            .await
            .map_err(|_| anyhow::anyhow!("Account store worker stopped before update admission"))?;
        response
            .await
            .context("Account store worker dropped an admitted update")?
    }
}

impl AccountStoreWorkerGuard {
    fn spawn(self) -> Result<AccountStoreRuntimeGuard> {
        let requests = self.control.clone();
        let worker = thread::Builder::new()
            .name("account-store-writer".to_string())
            .spawn(move || self.run())
            .context("Failed to start account store worker")?;
        Ok(AccountStoreRuntimeGuard {
            requests,
            worker: Some(worker),
        })
    }

    fn run(mut self) {
        while let Some(request) = self.requests.blocking_recv() {
            match request {
                WorkerRequest::Update(update) => {
                    update.execute(&mut self.current, &self.snapshot, &self.path)
                }
                WorkerRequest::Shutdown => {
                    self.requests.close();
                    while let Some(request) = self.requests.blocking_recv() {
                        if let WorkerRequest::Update(update) = request {
                            update.execute(&mut self.current, &self.snapshot, &self.path);
                        }
                    }
                    return;
                }
            }
        }
    }
}

impl Drop for AccountStoreRuntimeGuard {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let requests = self.requests.clone();
        let _ = thread::Builder::new()
            .name("account-store-shutdown".to_string())
            .spawn(move || {
                let _ = requests.blocking_send(WorkerRequest::Shutdown);
                let _ = worker.join();
            });
    }
}

impl AccountStoreRuntimeGuard {
    /// Drain every admitted update, stop the worker, and observe its exit.
    pub(crate) fn shutdown(mut self) -> Result<()> {
        self.requests
            .blocking_send(WorkerRequest::Shutdown)
            .map_err(|_| anyhow::anyhow!("Account store worker stopped before shutdown"))?;
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("Account store worker panicked"))?;
        }
        Ok(())
    }
}

impl<Update, Output> ErasedUpdate for TypedUpdate<Update, Output>
where
    Output: Send + 'static,
    Update: FnOnce(&mut AccountsStore) -> Result<Output> + Send + 'static,
{
    fn execute(
        self: Box<Self>,
        current: &mut AccountsStore,
        snapshot: &RwLock<AccountsStore>,
        path: &Path,
    ) {
        let mut candidate = current.clone();
        let result = (self.update)(&mut candidate).and_then(|output| {
            write_store_atomically(path, &candidate)?;
            *current = candidate.clone();
            let mut published = snapshot.write().map_err(|error| {
                anyhow::anyhow!("Account store snapshot lock is poisoned: {error}")
            })?;
            *published = candidate;
            Ok(output)
        });
        let _ = self.reply.send(result);
    }
}

pub(crate) fn initialize_default_store(path: PathBuf) -> Result<AccountStoreRuntimeGuard> {
    let (client, worker) = AccountStoreClient::open(path)?;
    DEFAULT_CLIENT
        .set(client)
        .map_err(|_| anyhow::anyhow!("Account store is already initialized"))?;
    worker.spawn()
}

pub(crate) fn default_client() -> Result<&'static AccountStoreClient> {
    DEFAULT_CLIENT
        .get()
        .context("Account store is not initialized")
}

fn read_store(path: &Path) -> Result<AccountsStore> {
    if !path.exists() {
        return Ok(AccountsStore::default());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))
}

fn write_store_atomically(path: &Path, store: &AccountsStore) -> Result<()> {
    let parent = path
        .parent()
        .context("Accounts file has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;

    let content = serde_json::to_vec_pretty(store).context("Failed to serialize accounts store")?;
    let mut temporary = NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "Failed to create temporary accounts file in {}",
            parent.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(&content)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace accounts file: {}", path.display()))?;

    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serializes_concurrent_updates_without_losing_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.json");
        let (client, worker) = AccountStoreClient::open(path.clone()).unwrap();
        let runtime = worker.spawn().unwrap();

        let mut tasks = Vec::new();
        for index in 0..32 {
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                client
                    .update(move |store| {
                        store.masked_account_ids.push(format!("account-{index}"));
                        Ok(())
                    })
                    .await
                    .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        let store: AccountsStore =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(store.masked_account_ids.len(), 32);
        drop(client);
        drop(runtime);
    }

    #[tokio::test]
    async fn executes_updates_on_the_guard_owned_worker_thread() {
        let directory = tempfile::tempdir().unwrap();
        let (client, worker) =
            AccountStoreClient::open(directory.path().join("accounts.json")).unwrap();
        let runtime = worker.spawn().unwrap();
        let caller = thread::current().id();

        let worker_thread = client.update(|_| Ok(thread::current().id())).await.unwrap();

        assert_ne!(caller, worker_thread);
        drop(client);
        drop(runtime);
    }
}
