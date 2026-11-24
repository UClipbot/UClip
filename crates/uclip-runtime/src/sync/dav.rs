use std::{io::Read, time::Duration};

use quick_xml::{events::Event, Reader};
use reqwest::{
    blocking::{Client, RequestBuilder, Response},
    Method, StatusCode,
};
use uclip_core::{AppError, ErrorCode};
use url::Url;
use uuid::Uuid;

use super::{
    archive::{ArchiveStore, Area},
    transport::{invalid, limit, unavailable},
};

const LIST_LIMIT: usize = 2 * 1024 * 1024;

pub(super) struct Dav {
    client: Client,
    root: Url,
    username: String,
    password: String,
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
        let this = Self {
            client,
            root,
            username: username.into(),
            password: password.into(),
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
        let temp = self.object(area, &format!("pending-{}", Uuid::new_v4()))?;
        let response = self
            .request("PUT", temp.clone())?
            .body(data)
            .send()
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let moved = self
            .request("MOVE", temp.clone())?
            .header("Destination", self.object(area, name)?.as_str())
            .header("Overwrite", "F")
            .send();
        match moved {
            Ok(response)
                if matches!(
                    response.status(),
                    StatusCode::CREATED | StatusCode::NO_CONTENT
                ) =>
            {
                Ok(true)
            }
            response => {
                if let Ok(request) = self.request("DELETE", temp) {
                    let _ = request.send();
                }
                match response {
                    Ok(response) if response.status() == StatusCode::PRECONDITION_FAILED => {
                        Ok(false)
                    }
                    Ok(response) => Err(status_error(response.status())),
                    Err(_) => Err(unavailable()),
                }
            }
        }
    }

    fn remove_manifest(&mut self, name: &str) {
        if let Ok(request) = self
            .object(Area::Manifests, name)
            .and_then(|url| self.request("DELETE", url))
        {
            let _ = request.send();
        }
    }
}
