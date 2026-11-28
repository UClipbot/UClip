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
    archive::{
        read_parallel, ArchiveFile, ArchiveStore, Area, FileRevision, ObjectRead, ObjectUpload,
        PARALLEL_READS,
    },
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
    read_batch_limit: usize,
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
            read_batch_limit: PARALLEL_READS,
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

    fn get(&self, area: Area, name: &str, bound: usize) -> ObjectRead {
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

fn read_objects_with(
    read_batch_limit: &mut usize,
    names: &[String],
    stopped: Option<&AtomicBool>,
    get: impl Fn(&str) -> ObjectRead + Sync,
) -> Vec<ObjectRead> {
    let mut results = Vec::with_capacity(names.len());
    let mut offset = 0;
    while offset < names.len() {
        if is_stopped(stopped) {
            results.extend((offset..names.len()).map(|_| Err(unavailable())));
            break;
        }
        let width = (*read_batch_limit).max(1).min(names.len() - offset);
        let chunk = &names[offset..offset + width];
        let mut batch = read_parallel(chunk, stopped, |name| get(name));
        let mut aborted = is_stopped(stopped);
        let has_connection_error = batch
            .iter()
            .any(|result| matches!(result, Err(error) if error.code == ErrorCode::SyncConnection));
        if !aborted && chunk.len() > 1 && has_connection_error {
            // A server may refuse concurrent requests. Retry those reads serially.
            *read_batch_limit = 1;
            for (name, result) in chunk.iter().zip(&mut batch) {
                if is_stopped(stopped) {
                    aborted = true;
                    break;
                }
                if matches!(result, Err(error) if error.code == ErrorCode::SyncConnection) {
                    *result = get(name);
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

fn listing(xml: &[u8], area: Area) -> Result<Vec<ArchiveFile>, AppError> {
    #[derive(Default)]
    struct ResponseFile {
        href: String,
        etag: String,
        modified: String,
        length: String,
    }

    #[derive(Clone, Copy)]
    enum Property {
        Href,
        Etag,
        Modified,
        Length,
    }

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut files = Vec::new();
    let mut file = ResponseFile::default();
    let mut property = None;
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(tag) => {
                property = match tag.local_name().as_ref() {
                    b"href" => Some(Property::Href),
                    b"getetag" => Some(Property::Etag),
                    b"getlastmodified" => Some(Property::Modified),
                    b"getcontentlength" => Some(Property::Length),
                    _ => None,
                };
            }
            Event::End(tag) => {
                if tag.local_name().as_ref() == b"response" {
                    let response = std::mem::take(&mut file);
                    if let Some(name) = response
                        .href
                        .rsplit('/')
                        .next()
                        .filter(|name| area.matches(name))
                    {
                        let validator = if response.etag.is_empty() {
                            response.modified
                        } else {
                            response.etag
                        };
                        files.push(ArchiveFile {
                            name: name.to_owned(),
                            revision: FileRevision {
                                size: response.length.parse().ok(),
                                validator: (!validator.is_empty()).then_some(validator),
                            },
                        });
                    }
                }
                property = None;
            }
            Event::Text(text) => {
                let value = match property {
                    Some(Property::Href) => &mut file.href,
                    Some(Property::Etag) => &mut file.etag,
                    Some(Property::Modified) => &mut file.modified,
                    Some(Property::Length) => &mut file.length,
                    None => continue,
                };
                value.push_str(&text.decode().map_err(|_| invalid())?);
            }
            Event::GeneralRef(reference) if matches!(property, Some(Property::Etag)) => {
                // ETags may contain XML-escaped quotes. Keep their representation stable.
                file.etag.push('&');
                file.etag
                    .push_str(&reference.decode().map_err(|_| invalid())?);
                file.etag.push(';');
            }
            Event::DocType(_) => return Err(invalid()),
            Event::Eof => return Ok(files),
            _ => {}
        }
    }
}

impl ArchiveStore for Dav {
    fn list(&mut self, area: Area) -> Result<Vec<ArchiveFile>, AppError> {
        let properties = match area {
            Area::Manifests => "<d:resourcetype/>",
            Area::Objects => {
                "<d:resourcetype/><d:getetag/><d:getlastmodified/><d:getcontentlength/>"
            }
        };
        let response = self
            .request("PROPFIND", self.area(area)?)?
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(format!(
                r#"<d:propfind xmlns:d="DAV:"><d:prop>{properties}</d:prop></d:propfind>"#
            ))
            .send()
            .map_err(|_| unavailable())?;
        if response.status() != StatusCode::MULTI_STATUS {
            return Err(status_error(response.status()));
        }
        let limit = match area {
            Area::Manifests => LIST_LIMIT,
            Area::Objects => 32 * 1024 * 1024,
        };
        listing(&read_response(response, limit)?, area)
    }

    fn object_revision(&mut self, name: &str) -> Result<Option<FileRevision>, AppError> {
        use reqwest::header::{CONTENT_LENGTH, ETAG, LAST_MODIFIED};

        let response = self
            .request("HEAD", self.object(Area::Objects, name)?)?
            .send()
            .map_err(|_| unavailable())?;
        match response.status() {
            StatusCode::OK => {
                let headers = response.headers();
                let size = headers
                    .get(CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse().ok());
                let validator = headers
                    .get(ETAG)
                    .or_else(|| headers.get(LAST_MODIFIED))
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                Ok(Some(FileRevision { size, validator }))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(status_error(status)),
        }
    }

    fn read(&mut self, area: Area, name: &str, bound: usize) -> Result<Option<Vec<u8>>, AppError> {
        self.get(area, name, bound)
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
        let mut limit = self.read_batch_limit;
        let results = read_objects_with(&mut limit, names, stopped, |name| {
            self.get(Area::Objects, name, bound)
        });
        self.read_batch_limit = limit;
        results
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

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    };

    use super::*;

    #[test]
    fn listing_keeps_object_revisions_associated_with_their_paths() {
        let files = listing(
            br#"<d:multistatus xmlns:d="DAV:">
                <d:response><d:href>/objects/first.pack</d:href><d:propstat><d:prop>
                    <d:getetag>&quot;first-tag&quot;</d:getetag>
                    <d:getlastmodified>first-date</d:getlastmodified>
                    <d:getcontentlength>42</d:getcontentlength>
                </d:prop></d:propstat></d:response>
                <d:response><d:href>/objects/second.clip</d:href><d:propstat><d:prop>
                    <d:getetag/><d:getlastmodified>second-date</d:getlastmodified>
                </d:prop></d:propstat></d:response>
                <d:response><d:href>/objects/pending-file</d:href></d:response>
            </d:multistatus>"#,
            Area::Objects,
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "first.pack");
        assert_eq!(files[0].revision.size, Some(42));
        assert_eq!(
            files[0].revision.validator.as_deref(),
            Some("&quot;first-tag&quot;")
        );
        assert_eq!(files[1].name, "second.clip");
        assert_eq!(files[1].revision.size, None);
        assert_eq!(files[1].revision.validator.as_deref(), Some("second-date"));
    }

    #[test]
    fn parallel_reads_stay_bounded_and_fall_back_to_serial_on_connection_errors() {
        let names: Vec<_> = (0..6).map(|i| format!("{i}.clip")).collect();
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let calls = AtomicUsize::new(0);
        let barrier = Barrier::new(2);
        let mut limit = 2;

        let results = read_objects_with(&mut limit, &names, None, |name| {
            let attempt = calls.fetch_add(1, Ordering::AcqRel);
            let current = active.fetch_add(1, Ordering::AcqRel) + 1;
            peak.fetch_max(current, Ordering::AcqRel);
            let result = if attempt < 2 {
                // Force both reads in the first batch to overlap before simulating server rejection.
                barrier.wait();
                Err(unavailable())
            } else {
                assert_eq!(current, 1);
                Ok(Some(name.as_bytes().to_vec()))
            };
            active.fetch_sub(1, Ordering::AcqRel);
            result
        });

        assert_eq!(limit, 1);
        assert_eq!(peak.load(Ordering::Acquire), 2);
        let values: Vec<_> = results
            .into_iter()
            .map(|result| result.unwrap().unwrap())
            .collect();
        assert_eq!(
            values,
            names
                .iter()
                .map(|name| name.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn serial_connection_failure_stops_without_retrying_or_reading_remaining_objects() {
        let names = ["a.clip".into(), "b.clip".into(), "c.clip".into()];
        let attempts = AtomicUsize::new(0);
        let mut limit = 1;

        let results = read_objects_with(&mut limit, &names, None, |_| {
            attempts.fetch_add(1, Ordering::AcqRel);
            Err(unavailable())
        });

        assert_eq!(attempts.load(Ordering::Acquire), 1);
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|result| matches!(result, Err(error) if error.code == ErrorCode::SyncConnection)));
    }
}
