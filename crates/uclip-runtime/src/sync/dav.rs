mod bulk;

use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use quick_xml::{events::Event, Reader};
use reqwest::{
    blocking::{Client, RequestBuilder, Response},
    Method, StatusCode,
};
use uclip_core::{AppError, ErrorCode};
use url::Url;
use uuid::Uuid;

use super::{
    archive::{ArchiveStore, Area, ObjectUpload},
    transport::{invalid, limit, unavailable},
};

const LIST_LIMIT: usize = 2 * 1024 * 1024;
const BATCH_FILES: usize = 64;
const PARALLEL_WRITES: usize = 4;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Dav {
    client: Client,
    root: Url,
    username: String,
    password: String,
    bulk: Option<bulk::Nextcloud>,
    bulk_available: Option<bool>,
}

pub(super) fn endpoint(raw: &str) -> Result<Url, AppError> {
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(AppError::new(ErrorCode::InvalidInput));
    }
    Ok(url)
}

impl Dav {
    pub fn connect(
        raw: &str,
        username: &str,
        password: &str,
        namespace: &str,
    ) -> Result<Self, AppError> {
        let mut root = endpoint(raw)?;
        root.path_segments_mut()
            .map_err(|_| invalid())?
            .pop_if_empty()
            .push(namespace)
            .push("");
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(45))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| unavailable())?;
        let bulk = bulk::Nextcloud::from_root(&root, username);
        let this = Self {
            client,
            root,
            username: username.into(),
            password: password.into(),
            bulk,
            bulk_available: None,
        };
        this.directory(this.root.clone())?;
        for area in [Area::Manifests, Area::Objects] {
            this.directory(this.area(area)?)?;
        }
        Ok(this)
    }

    fn area(&self, area: Area) -> Result<Url, AppError> {
        self.root
            .join(&format!("{}/", area.name()))
            .map_err(|_| invalid())
    }

    fn object(&self, area: Area, name: &str) -> Result<Url, AppError> {
        self.area(area)?.join(name).map_err(|_| invalid())
    }

    fn request(&self, method: &str, url: Url) -> Result<RequestBuilder, AppError> {
        let method = Method::from_bytes(method.as_bytes()).map_err(|_| invalid())?;
        let request = self.client.request(method, url);
        if self.username.is_empty() {
            return Ok(request);
        }
        Ok(request.basic_auth(&self.username, Some(&self.password)))
    }

    fn directory(&self, url: Url) -> Result<(), AppError> {
        let response = self
            .request("MKCOL", url)?
            .send()
            .map_err(|_| unavailable())?;
        if response.status().is_success() || response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        Err(status_error(response.status()))
    }

    fn discard(&self, url: Url) {
        if let Ok(request) = self.request("DELETE", url) {
            let _ = request.timeout(CLEANUP_TIMEOUT).send();
        }
    }

    fn move_object(&self, temporary: &Url, destination: &Url) -> Result<StatusCode, AppError> {
        let response = self
            .request("MOVE", temporary.clone())?
            .header("Destination", destination.as_str())
            .header("Overwrite", "F")
            .send()
            .map_err(|_| unavailable())?;
        Ok(response.status())
    }

    fn create_one(&self, area: Area, name: &str, data: Vec<u8>) -> Result<bool, AppError> {
        let temporary = self.object(area, &format!("pending-{}", Uuid::new_v4()))?;
        let destination = self.object(area, name)?;
        let result = (|| {
            let response = self
                .request("PUT", temporary.clone())?
                .body(data)
                .send()
                .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(status_error(response.status()));
            }
            move_result(self.move_object(&temporary, &destination)?)
        })();
        if !matches!(result, Ok(true)) {
            self.discard(temporary);
        }
        result
    }

    fn finalize_objects(
        &self,
        objects: Vec<ObjectUpload>,
        staged: Option<&bulk::Staged>,
        stopped: Option<&AtomicBool>,
    ) -> Vec<Result<bool, AppError>> {
        let mut remaining = objects.into_iter().enumerate();
        let mut results = Vec::new();
        let mut failure = None;
        while remaining.len() > 0 && failure.is_none() && !is_stopped(stopped) {
            let completed = thread::scope(|scope| {
                let jobs: Vec<_> = remaining
                    .by_ref()
                    .take(PARALLEL_WRITES)
                    .map(|(index, object)| {
                        thread::Builder::new().spawn_scoped(scope, move || {
                            if is_stopped(stopped) {
                                return Err(unavailable());
                            }
                            if let Some(staged) = staged.filter(|staged| staged.uploaded[index]) {
                                let temporary =
                                    staged.directory.join(&object.name).map_err(|_| invalid())?;
                                let status = self.move_object(
                                    &temporary,
                                    &self.object(Area::Objects, &object.name)?,
                                )?;
                                if status != StatusCode::NOT_FOUND {
                                    return move_result(status);
                                }
                            }
                            if is_stopped(stopped) {
                                return Err(unavailable());
                            }
                            self.create_one(Area::Objects, &object.name, object.data)
                        })
                    })
                    .collect();
                jobs.into_iter()
                    .map(|job| {
                        job.map_err(|_| unavailable())?
                            .join()
                            .map_err(|_| unavailable())?
                    })
                    .collect::<Vec<_>>()
            });
            failure = completed.iter().find_map(|result| match result {
                Err(error)
                    if !matches!(error.code, ErrorCode::SyncData | ErrorCode::LimitExceeded) =>
                {
                    Some(error.clone())
                }
                _ => None,
            });
            results.extend(completed);
        }
        results.extend(remaining.map(|_| Err(failure.clone().unwrap_or_else(unavailable))));
        results
    }
}

fn is_stopped(stopped: Option<&AtomicBool>) -> bool {
    stopped.is_some_and(|flag| flag.load(Ordering::Acquire))
}

fn move_result(status: StatusCode) -> Result<bool, AppError> {
    match status {
        StatusCode::CREATED | StatusCode::NO_CONTENT => Ok(true),
        StatusCode::PRECONDITION_FAILED => Ok(false),
        status => Err(status_error(status)),
    }
}

fn status_error(status: StatusCode) -> AppError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        AppError::new(ErrorCode::SyncAuthentication)
    } else if status == StatusCode::PAYLOAD_TOO_LARGE {
        limit()
    } else {
        unavailable()
    }
}

fn read_response(response: Response, bound: usize) -> Result<Vec<u8>, AppError> {
    if response
        .content_length()
        .is_some_and(|size| size > bound as u64)
    {
        return Err(limit());
    }
    let mut data = Vec::new();
    response
        .take(bound as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| unavailable())?;
    if data.len() > bound {
        return Err(limit());
    }
    Ok(data)
}

fn listing(xml: &[u8], suffix: &str) -> Result<Vec<String>, AppError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut names = Vec::new();
    let mut href = false;
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(tag) if tag.local_name().as_ref() == b"href" => href = true,
            Event::End(tag) if tag.local_name().as_ref() == b"href" => href = false,
            Event::Text(text) if href => {
                let path = text.decode().map_err(|_| invalid())?;
                if let Some(name) = path
                    .rsplit('/')
                    .next()
                    .filter(|name| name.ends_with(suffix))
                {
                    names.push(name.to_string());
                }
            }
            Event::DocType(_) => return Err(invalid()),
            Event::Eof => return Ok(names),
            _ => {}
        }
    }
}

impl ArchiveStore for Dav {
    fn list(&mut self, area: Area) -> Result<Vec<String>, AppError> {
        let response = self
            .request("PROPFIND", self.area(area)?)?
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(r#"<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#)
            .send()
            .map_err(|_| unavailable())?;
        if response.status() != StatusCode::MULTI_STATUS {
            return Err(status_error(response.status()));
        }
        let limit = match area {
            Area::Manifests => LIST_LIMIT,
            Area::Objects => 32 * 1024 * 1024,
        };
        listing(&read_response(response, limit)?, area.suffix())
    }

    fn object_exists(&mut self, name: &str) -> Result<bool, AppError> {
        let response = self
            .request("HEAD", self.object(Area::Objects, name)?)?
            .send()
            .map_err(|_| unavailable())?;
        match response.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(status_error(status)),
        }
    }

    fn read(&mut self, area: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError> {
        let response = self
            .request("GET", self.object(area, name)?)?
            .send()
            .map_err(|_| unavailable())?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        read_response(response, bound).map(Some)
    }

    fn create(&mut self, area: Area, name: &str, data: Vec<u8>) -> Result<bool, AppError> {
        self.create_one(area, name, data)
    }

    fn object_batch_size(&self) -> usize {
        BATCH_FILES
    }

    fn create_objects(
        &mut self,
        objects: Vec<ObjectUpload>,
        stopped: Option<&AtomicBool>,
    ) -> Vec<Result<bool, AppError>> {
        if is_stopped(stopped) {
            return objects.into_iter().map(|_| Err(unavailable())).collect();
        }
        let staged = if let Some(bulk) = self.bulk.as_ref().filter(|_| objects.len() > 1) {
            let available = self.bulk_available.unwrap_or_else(|| bulk.advertised(self));
            self.bulk_available = Some(available);
            if available && !is_stopped(stopped) {
                let staged = bulk.stage(self, &objects, stopped);
                if staged
                    .as_ref()
                    .is_none_or(|batch| !batch.uploaded.iter().any(|&ok| ok))
                {
                    self.bulk_available = Some(false);
                }
                staged
            } else {
                None
            }
        } else {
            None
        };
        let results = self.finalize_objects(objects, staged.as_ref(), stopped);
        if let Some(staged) = staged {
            self.discard(staged.directory);
        }
        results
    }

    fn remove_manifest(&mut self, name: &str) {
        if let Ok(url) = self.object(Area::Manifests, name) {
            self.discard(url);
        }
    }
}
