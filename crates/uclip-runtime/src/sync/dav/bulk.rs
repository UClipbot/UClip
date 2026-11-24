use std::sync::atomic::AtomicBool;

use percent_encoding::percent_decode_str;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{is_stopped, read_response, Area, Dav, ObjectUpload, StatusCode, Url, Uuid};
use crate::sync::{lan::now, transport::UPLOAD_BATCH_BYTES};

const CAPABILITIES_LIMIT: usize = 256 * 1024;
const BULK_RESPONSE_LIMIT: usize = 256 * 1024;
const MULTIPART_LIMIT: usize = UPLOAD_BATCH_BYTES + 256 * 1024;

pub(super) struct Nextcloud {
    endpoint: Url,
    capabilities: Url,
    directory: String,
}

pub(super) struct Staged {
    pub directory: Url,
    pub uploaded: Vec<bool>,
}

fn decode_segment(segment: &str) -> Option<String> {
    let value = percent_decode_str(segment).decode_utf8().ok()?;
    if value.is_empty()
        || matches!(value.as_ref(), "." | "..")
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.into_owned())
}

impl Nextcloud {
    pub(super) fn from_root(root: &Url, username: &str) -> Option<Self> {
        let (prefix, path) = root.path().split_once("/remote.php/dav/files/")?;
        let (account, path) = path.split_once('/')?;
        // Nextcloud interprets X-File-Path relative to the authenticated user's files.
        if decode_segment(account)? != username {
            return None;
        }
        let path = path
            .trim_end_matches('/')
            .split('/')
            .map(decode_segment)
            .collect::<Option<Vec<_>>>()?
            .join("/");
        let mut endpoint = root.clone();
        endpoint.set_path(&format!("{prefix}/remote.php/dav/bulk"));
        let mut capabilities = root.clone();
        capabilities.set_path(&format!("{prefix}/ocs/v2.php/cloud/capabilities"));
        capabilities.set_query(Some("format=json"));
        Some(Self {
            endpoint,
            capabilities,
            directory: format!("/{path}/objects"),
        })
    }

    pub(super) fn advertised(&self, dav: &Dav) -> bool {
        let response = dav
            .request("GET", self.capabilities.clone())
            .ok()
            .and_then(|request| request.header("OCS-APIRequest", "true").send().ok())
            .filter(|response| response.status().is_success());
        response
            .and_then(|response| read_response(response, CAPABILITIES_LIMIT).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|data| {
                data.pointer("/ocs/data/capabilities/dav/bulkupload")
                    .and_then(Value::as_str)
                    == Some("1.0")
            })
    }

    pub(super) fn stage(
        &self,
        dav: &Dav,
        objects: &[ObjectUpload],
        stopped: Option<&AtomicBool>,
    ) -> Option<Staged> {
        let folder = format!("pending-{}", Uuid::new_v4());
        let directory = dav
            .area(Area::Objects)
            .ok()?
            .join(&format!("{folder}/"))
            .ok()?;
        let paths: Vec<_> = objects
            .iter()
            .map(|object| format!("{}/{folder}/{}", self.directory, object.name))
            .collect();
        let boundary = format!("uclip-{}", Uuid::new_v4());
        let body = encode_parts(&boundary, &paths, objects)?;
        if is_stopped(stopped) {
            return None;
        }
        let response = dav.request("MKCOL", directory.clone()).ok()?.send().ok()?;
        if response.status() != StatusCode::CREATED {
            return None;
        }
        if is_stopped(stopped) {
            dav.discard(directory);
            return None;
        }
        let response = dav
            .request("POST", self.endpoint.clone())
            .ok()
            .and_then(|request| {
                request
                    .header(
                        "Content-Type",
                        format!("multipart/related; boundary={boundary}"),
                    )
                    .body(body)
                    .send()
                    .ok()
            });
        // A 400 can report parts committed before a parse error. Treat every
        // absent or unsuccessful part as unconfirmed, including on connection loss.
        let replies = response
            .filter(|response| {
                matches!(response.status(), StatusCode::OK | StatusCode::BAD_REQUEST)
            })
            .and_then(|response| read_response(response, BULK_RESPONSE_LIMIT).ok())
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            .unwrap_or_default();
        Some(Staged {
            directory,
            uploaded: confirmed_parts(&paths, &replies),
        })
    }
}

fn confirmed_parts(paths: &[String], replies: &Value) -> Vec<bool> {
    paths
        .iter()
        .map(|path| {
            replies.get(path).is_some_and(|part| {
                part.get("error").and_then(Value::as_bool) == Some(false)
                    && part
                        .get("etag")
                        .and_then(Value::as_str)
                        .is_some_and(|etag| !etag.is_empty())
            })
        })
        .collect()
}

fn encode_parts(boundary: &str, paths: &[String], objects: &[ObjectUpload]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mtime = now();
    for (path, object) in paths.iter().zip(objects) {
        let checksum = Sha256::digest(&object.data);
        let headers = format!(
            "--{boundary}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nOC-Checksum: sha256:{checksum:x}\r\nX-OC-Mtime: {mtime}\r\nX-File-Path: {path}\r\n\r\n",
            object.data.len(),
        );
        if body.len() + headers.len() + object.data.len() + 2 > MULTIPART_LIMIT {
            return None;
        }
        body.extend_from_slice(headers.as_bytes());
        body.extend_from_slice(&object.data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body.len() <= MULTIPART_LIMIT).then_some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_acknowledgement_does_not_discard_other_confirmed_parts() {
        let paths = ["/first", "/invalid", "/third", "/absent"].map(str::to_string);
        let replied = serde_json::json!({
            "/first": {"error": false, "etag": "first"},
            "/invalid": {"error": "unknown", "etag": "second"},
            "/third": {"error": false, "etag": "third"},
            "/unsolicited": {"error": false, "etag": "unrequested"}
        });
        assert_eq!(
            confirmed_parts(&paths, &replied),
            [true, false, true, false]
        );
    }

    #[test]
    fn only_the_authenticated_nextcloud_files_root_is_eligible_for_bulk() {
        let root = Url::parse(
            "https://example.test:8443/cloud/remote.php/dav/files/alice/My%20history/uclip-v1-test/",
        ).unwrap();
        let bulk = Nextcloud::from_root(&root, "alice").unwrap();
        assert_eq!(
            bulk.endpoint.as_str(),
            "https://example.test:8443/cloud/remote.php/dav/bulk"
        );
        assert_eq!(
            bulk.capabilities.path(),
            "/cloud/ocs/v2.php/cloud/capabilities"
        );
        assert_eq!(bulk.directory, "/My history/uclip-v1-test/objects");
        assert!(Nextcloud::from_root(&root, "bob").is_none());
        for segment in ["bad%2Fpath", "bad%5Cpath", "bad%0d%0Apath", "bad%00path"] {
            assert!(decode_segment(segment).is_none(), "{segment}");
        }
    }

    #[test]
    fn multipart_is_binary_safe_and_bounded() {
        let objects = [ObjectUpload {
            name: "sample.clip".into(),
            data: b"first\r\n\0second".to_vec(),
        }];
        let paths = ["/history/objects/sample.clip".into()];
        let body = encode_parts("example", &paths, &objects).unwrap();
        let headers_end = body
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&body[..headers_end]).unwrap();
        assert!(headers.contains("Content-Length: 14\r\n"));
        assert!(headers.contains(&format!(
            "OC-Checksum: sha256:{:x}\r\n",
            Sha256::digest(&objects[0].data)
        )));
        assert!(headers.ends_with("X-File-Path: /history/objects/sample.clip"));
        assert_eq!(
            &body[headers_end + 4..],
            b"first\r\n\0second\r\n--example--\r\n"
        );
        let large = [ObjectUpload {
            name: "sample.clip".into(),
            data: vec![0; MULTIPART_LIMIT],
        }];
        assert!(encode_parts("example", &paths, &large).is_none());
    }
}
