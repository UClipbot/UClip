//! X11 clipboard provider identity. Never infer provenance from the active window.
use std::collections::BTreeSet;
use x11rb::{
    protocol::{
        res,
        xproto::{AtomEnum, ConnectionExt, Window},
    },
    rust_connection::RustConnection,
    NONE,
};

fn atom(conn: &RustConnection, name: &[u8]) -> Option<u32> {
    Some(conn.intern_atom(true, name).ok()?.reply().ok()?.atom).filter(|a| *a != NONE)
}
fn window_property(
    conn: &RustConnection,
    window: Window,
    property: u32,
    kind: AtomEnum,
) -> Option<u32> {
    let reply = conn
        .get_property(false, window, property, kind, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    if reply.bytes_after != 0 || reply.value_len != 1 {
        return None;
    }
    let value = reply.value32()?.next();
    value.filter(|value| *value != 0)
}
fn class_name(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 512 || bytes.last() != Some(&0) {
        return None;
    }
    let mut fields = bytes.split(|byte| *byte == 0);
    let instance = fields.next()?;
    let class = fields.next()?;
    // ICCCM WM_CLASS consists of exactly two NUL-terminated strings.
    if fields.next() != Some(&[][..]) || fields.next().is_some() {
        return None;
    }
    for value in [class, instance] {
        let Ok(name) = std::str::from_utf8(value) else {
            continue;
        };
        let name = name.trim();
        if !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control) {
            return Some(name.to_owned());
        }
    }
    None
}
fn window_class(conn: &RustConnection, window: Window) -> Option<String> {
    let reply = conn
        .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 128)
        .ok()?
        .reply()
        .ok()?;
    if reply.format != 8 || reply.bytes_after != 0 {
        return None;
    }
    class_name(&reply.value)
}
fn unique_class(conn: &RustConnection, windows: impl Iterator<Item = Window>) -> Option<String> {
    let names: BTreeSet<_> = windows
        .filter_map(|window| window_class(conn, window))
        .collect();
    if names.len() == 1 {
        names.into_iter().next()
    } else {
        None
    }
}

pub fn application(conn: &RustConnection, root: Window, owner: Window) -> Option<String> {
    if owner == NONE {
        return None;
    }
    if let Some(name) = window_class(conn, owner) {
        return Some(name);
    }
    let leader = atom(conn, b"WM_CLIENT_LEADER")
        .and_then(|property| window_property(conn, owner, property, AtomEnum::WINDOW));
    if let Some(name) = leader
        .filter(|leader| *leader != owner)
        .and_then(|leader| window_class(conn, leader))
    {
        return Some(name);
    }
    // Chrome and other toolkits use anonymous owner windows. XRes links them to
    // the actual X client; only inspect a bounded set of that client's app windows.
    let clients = res::query_clients(conn).ok()?.reply().ok()?.clients;
    let client = clients
        .into_iter()
        .find(|client| owner & !client.resource_mask == client.resource_base)?;
    let property = atom(conn, b"_NET_CLIENT_LIST")?;
    let reply = conn
        .get_property(false, root, property, AtomEnum::WINDOW, 0, 128)
        .ok()?
        .reply()
        .ok()?;
    let windows: Vec<_> = reply.value32()?.collect();
    let owned: Vec<_> = windows
        .iter()
        .copied()
        .filter(|window| window & !client.resource_mask == client.resource_base)
        .collect();
    if !owned.is_empty() {
        return unique_class(conn, owned.into_iter());
    }

    // An application may use multiple X connections. XRes 1.2's server-reported
    // client PID can bridge them without looking up local processes or paths.
    let version = res::query_version(conn, 1, 2).ok()?.reply().ok()?;
    if (version.server_major, version.server_minor) < (1, 2) {
        return None;
    }
    let reply = res::query_client_ids(
        conn,
        &[res::ClientIdSpec {
            client: owner,
            mask: res::ClientIdMask::LOCAL_CLIENT_PID,
        }],
    )
    .ok()?
    .reply()
    .ok()?;
    let pid = reply
        .ids
        .iter()
        .find_map(|entry| {
            (entry.spec.mask == res::ClientIdMask::LOCAL_CLIENT_PID && entry.value.len() == 1)
                .then(|| entry.value[0])
        })
        .filter(|pid| *pid != 0)?;
    let pid_atom = atom(conn, b"_NET_WM_PID")?;
    let candidates = windows
        .into_iter()
        .filter(|window| window_property(conn, *window, pid_atom, AtomEnum::CARDINAL) == Some(pid));
    unique_class(conn, candidates)
}

#[cfg(test)]
mod tests {
    use super::class_name;
    #[test]
    fn names_are_bounded_and_do_not_include_window_titles() {
        assert_eq!(
            class_name(b"google-chrome\0Google-chrome\0").as_deref(),
            Some("Google-chrome")
        );
        assert_eq!(class_name(b"chromium\0\0").as_deref(), Some("chromium"));
        for invalid in [
            b"\0\0".as_slice(),
            b"x\0name\nsecret\0",
            b"x\0name",
            b"x\0y\0extra\0",
            b"x\0\xff\0",
        ] {
            // An invalid class may fall back only to a valid instance, never raw bytes.
            let result = class_name(invalid);
            assert!(result.is_none() || result.as_deref() == Some("x"));
        }
        assert!(class_name(&vec![b'x'; 513]).is_none());
    }
}
