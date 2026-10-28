//! One native process owns a library. Subsequent launches request its window.
use anyhow::{Context, Result};
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};
/// Installers hold this lock exclusively while replacing the app or library.
/// The reader holds it shared for its whole life, before claiming `native.lock`.
pub struct MaintenanceGuard(File);
impl MaintenanceGuard {
    pub fn claim(data: &Path) -> Result<Self> {
        let name = data
            .file_name()
            .context("Data directory needs a name")?
            .to_string_lossy();
        let path = data
            .parent()
            .context("Data parent missing")?
            .join(format!(".{name}.cutover.lock"));
        if let Ok(m) = std::fs::symlink_metadata(&path) {
            anyhow::ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "Invalid maintenance lock"
            );
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        FileExt::try_lock_shared(&file).context("Native storage maintenance is running")?;
        Ok(Self(file))
    }
}
impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
type Waker = Arc<dyn Fn() + Send + Sync>;
type SharedWaker = Arc<Mutex<Option<Waker>>>;
pub struct Instance {
    _lock: File,
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    pub events: Receiver<()>,
    wake: SharedWaker,
}
impl Instance {
    pub fn claim(data: &Path) -> Result<Option<Self>> {
        let lock_path = data.join("native.lock");
        if let Ok(m) = std::fs::symlink_metadata(&lock_path) {
            anyhow::ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "Invalid native process lock"
            );
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(lock_path)?;
        let socket = data.join("native.socket");
        match lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let mut stream = UnixStream::connect(&socket)
                    .context("Native app is starting; open it again in a moment")?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                stream.write_all(b"show\n")?;
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        }
        if socket.exists() {
            std::fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let (tx, events) = mpsc::sync_channel(4);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let wake: SharedWaker = Arc::new(Mutex::new(None));
        let notify = wake.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut client, _)) => {
                        let _ = client.set_read_timeout(Some(Duration::from_millis(100)));
                        let mut b = [0u8; 5];
                        if client.read_exact(&mut b).is_ok() && &b == b"show\n" {
                            let _ = tx.try_send(());
                            if let Ok(w) = notify.lock()
                                && let Some(w) = w.as_ref()
                            {
                                w();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Some(Self {
            _lock: lock,
            socket,
            stop,
            worker: Some(worker),
            events,
            wake,
        }))
    }
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake.lock().expect("Instance wake lock") = Some(wake);
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the blocking listener without polling while the app is idle.
        if let Ok(mut client) = UnixStream::connect(&self.socket) {
            let _ = client.set_write_timeout(Some(Duration::from_millis(100)));
            let _ = client.write_all(b"stop\n");
        }
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_launch_reopens_owner_and_shutdown_releases_socket() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let owner = Instance::claim(directory.path()).unwrap().unwrap();
        assert!(Instance::claim(directory.path()).unwrap().is_none());
        owner.events.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(owner);
        assert!(!directory.path().join("native.socket").exists());
        let next = Instance::claim(directory.path()).unwrap().unwrap();
        drop(next);
    }
}
