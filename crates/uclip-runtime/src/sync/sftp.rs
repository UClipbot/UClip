use std::{
    io::{Read, Write},
    iter,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use ssh2::{OpenFlags, OpenType, RenameFlags, Session, Sftp};
use uclip_core::{AppError, ErrorCode};
use uuid::Uuid;

use super::{
    archive::{
        read_parallel, ArchiveFile, ArchiveStore, Area, FileRevision, ObjectRead, PARALLEL_READS,
    },
    transport::{invalid, limit, unavailable},
};

const DEADLINE: Duration = Duration::from_secs(8);

pub(super) struct Ssh {
    channel: Sftp,
    root: PathBuf,
    login: Login,
    readers: Vec<Sftp>,
    read_batch_limit: usize,
}

struct Login {
    host: String,
    port: u16,
    username: String,
    password: String,
    expected_key: Vec<u8>,
}

pub(super) fn fingerprint(fingerprint: &str) -> Result<Vec<u8>, AppError> {
    let digest = STANDARD_NO_PAD
        .decode(fingerprint.strip_prefix("SHA256:").ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    if digest.len() != 32 {
        return Err(invalid());
    }
    Ok(digest)
}

impl Login {
    fn channel(&self) -> Result<Sftp, AppError> {
        let stream = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|_| unavailable())?
            .take(4)
            .find_map(|address| TcpStream::connect_timeout(&address, DEADLINE).ok())
            .ok_or_else(unavailable)?;
        let mut session = Session::new().map_err(|_| unavailable())?;
        session.set_timeout(DEADLINE.as_millis() as u32);
        session.set_tcp_stream(stream);
        session.handshake().map_err(|_| unavailable())?;
        let (key, _) = session.host_key().ok_or_else(unavailable)?;
        if &Sha256::digest(key)[..] != self.expected_key.as_slice() {
            return Err(AppError::new(ErrorCode::SyncAuthentication));
        }
        session
            .userauth_password(&self.username, &self.password)
            .map_err(|_| AppError::new(ErrorCode::SyncAuthentication))?;
        session.sftp().map_err(|_| unavailable())
    }
}

impl Ssh {
    pub fn connect(
        host: &str,
        port: u16,
        directory: &str,
        username: &str,
        password: &str,
        host_key: &str,
        namespace: &str,
    ) -> Result<Self, AppError> {
        let login = Login {
            host: host.into(),
            port,
            username: username.into(),
            password: password.into(),
            expected_key: fingerprint(host_key)?,
        };
        let channel = login.channel()?;
        let root = PathBuf::from(format!("{}/{namespace}", directory.trim_end_matches('/')));
        let this = Self {
            channel,
            root,
            login,
            readers: Vec::new(),
            read_batch_limit: PARALLEL_READS,
        };
        this.directory(&this.root)?;
        for area in [Area::Manifests, Area::Objects] {
            this.directory(&this.path(area, ""))?;
        }
        Ok(this)
    }

    fn path(&self, area: Area, name: &str) -> PathBuf {
        // SFTP server paths use '/' on every client OS.
        let root = self.root.as_os_str().to_string_lossy();
        PathBuf::from(format!("{root}/{}/{}", area.name(), name).trim_end_matches('/'))
    }

    fn directory(&self, path: &Path) -> Result<(), AppError> {
        match self.channel.mkdir(path, 0o700) {
            Ok(()) => Ok(()),
            Err(_) if self.channel.lstat(path).is_ok_and(|stat| stat.is_dir()) => Ok(()),
            Err(error) => Err(sftp_error(error)),
        }
    }

    fn get(channel: &Sftp, path: &Path, bound: usize) -> ObjectRead {
        let stat = match channel.lstat(path) {
            Ok(stat) if stat.is_file() => stat,
            Ok(_) => return Err(invalid()),
            Err(error) if error.code() == ssh2::ErrorCode::SFTP(2) => return Ok(None),
            Err(error) => return Err(sftp_error(error)),
        };
        if stat.size.is_some_and(|size| size > bound as u64) {
            return Err(limit());
        }
        let mut data = Vec::new();
        channel
            .open(path)
            .map_err(sftp_error)?
            .take(bound as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| unavailable())?;
        if data.len() > bound {
            return Err(limit());
        }
        Ok(Some(data))
    }
}

fn sftp_error(error: ssh2::Error) -> AppError {
    match error.code() {
        ssh2::ErrorCode::SFTP(3) => AppError::new(ErrorCode::SyncAuthentication),
        _ => unavailable(),
    }
}

fn file_revision(stat: ssh2::FileStat) -> FileRevision {
    FileRevision {
        size: stat.size,
        validator: stat.mtime.map(|modified| modified.to_string()),
    }
}

impl ArchiveStore for Ssh {
    fn list(&mut self, area: Area) -> Result<Vec<ArchiveFile>, AppError> {
        let mut directory = self
            .channel
            .opendir(self.path(area, ""))
            .map_err(sftp_error)?;
        let mut names = Vec::new();
        let limit_count = match area {
            Area::Manifests => 8192,
            Area::Objects => 150_000,
        };
        for _ in 0..limit_count {
            match directory.readdir() {
                Ok((name, stat)) if stat.is_file() => {
                    if let Some(name) = name
                        .file_name()
                        .and_then(|part| part.to_str())
                        .filter(|part| area.matches(part))
                    {
                        names.push(ArchiveFile {
                            name: name.to_owned(),
                            revision: file_revision(stat),
                        });
                    }
                }
                Ok(_) => {}
                // libssh2 marks end-of-directory with a LIBSSH2_ERROR_FILE code.
                Err(error) if error.code() == ssh2::ErrorCode::Session(-16) => return Ok(names),
                Err(error) => return Err(sftp_error(error)),
            }
        }
        Err(limit())
    }

    fn object_revision(&mut self, name: &str) -> Result<Option<FileRevision>, AppError> {
        match self.channel.lstat(&self.path(Area::Objects, name)) {
            Ok(stat) if stat.is_file() => Ok(Some(file_revision(stat))),
            Ok(_) => Err(invalid()),
            Err(error) if error.code() == ssh2::ErrorCode::SFTP(2) => Ok(None),
            Err(error) => Err(sftp_error(error)),
        }
    }

    fn read(&mut self, area: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError> {
        Self::get(&self.channel, &self.path(area, name), bound)
    }

    fn read_batch_size(&self) -> usize {
        self.read_batch_limit
    }

    fn read_objects(
        &mut self,
        names: &[String],
        bound: usize,
        stopped: Option<&AtomicBool>,
    ) -> Vec<ObjectRead> {
        let target = names.len().min(self.read_batch_limit);
        // Objects from the same blocking SSH session share a lock during I/O.
        while 1 + self.readers.len() < target
            && !stopped.is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            match self.login.channel() {
                Ok(reader) => self.readers.push(reader),
                Err(_) => {
                    self.read_batch_limit = 1 + self.readers.len();
                    break;
                }
            }
        }
        let mut results = Vec::with_capacity(names.len());
        let mut offset = 0;
        while offset < names.len() {
            if stopped.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                results.extend((offset..names.len()).map(|_| Err(unavailable())));
                break;
            }
            let width = (1 + self.readers.len()).min(names.len() - offset);
            let chunk = &names[offset..offset + width];
            let channels = iter::once(&self.channel).chain(&self.readers);
            let mut batch = read_parallel(channels.zip(chunk), stopped, |(channel, name)| {
                Self::get(channel, &self.path(Area::Objects, name), bound)
            });
            let mut aborted = stopped.is_some_and(|flag| flag.load(Ordering::Acquire));
            let has_connection_error = batch.iter().any(
                |result| matches!(result, Err(error) if error.code == ErrorCode::SyncConnection),
            );
            if !aborted && chunk.len() > 1 && has_connection_error {
                // A server may drop concurrent SFTP sessions. Drop extra channels and retry serially.
                self.readers.clear();
                self.read_batch_limit = 1;
                for (name, result) in chunk.iter().zip(&mut batch) {
                    if stopped.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                        aborted = true;
                        break;
                    }
                    if matches!(result, Err(error) if error.code == ErrorCode::SyncConnection) {
                        *result = Self::get(&self.channel, &self.path(Area::Objects, name), bound);
                        if matches!(result, Err(error) if error.code == ErrorCode::SyncConnection) {
                            aborted = true;
                            break;
                        }
                    }
                }
            } else if has_connection_error {
                aborted = true;
            }
            results.extend(batch);
            offset += chunk.len();
            if aborted {
                results.extend((offset..names.len()).map(|_| Err(unavailable())));
                break;
            }
        }
        results
    }

    fn create(&mut self, area: Area, name: &str, data: Vec<u8>) -> Result<bool, AppError> {
        let temp = self.path(area, &format!("pending-{}", Uuid::new_v4()));
        let destination = self.path(area, name);
        let mut file = self
            .channel
            .open_mode(
                &temp,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
                0o600,
                OpenType::File,
            )
            .map_err(sftp_error)?;
        let result = (|| {
            file.write_all(&data).map_err(|_| unavailable())?;
            // OpenSSH supports this extension. Flush and close precede publication.
            if let Err(error) = file.fsync() {
                if error.code() != ssh2::ErrorCode::SFTP(8) {
                    return Err(sftp_error(error));
                }
            }
            file.close().map_err(sftp_error)?;
            match self
                .channel
                .rename(&temp, &destination, Some(RenameFlags::empty()))
            {
                Ok(()) => Ok(true),
                Err(_)
                    if self
                        .channel
                        .lstat(&destination)
                        .is_ok_and(|stat| stat.is_file()) =>
                {
                    Ok(false)
                }
                Err(error) => Err(sftp_error(error)),
            }
        })();
        drop(file);
        if !matches!(result, Ok(true)) {
            let _ = self.channel.unlink(&temp);
        }
        result
    }

    fn remove_manifest(&mut self, name: &str) {
        let _ = self.channel.unlink(&self.path(Area::Manifests, name));
    }
}
