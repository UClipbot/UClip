use uclip_core::{CapabilityState as State, PlatformCapability, PlatformReport};

/// Environment is a hint, never proof of the GDK backend, portal support or permissions.
pub fn report() -> PlatformReport {
    let session_hint = if cfg!(target_os = "linux") {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        session_hint(
            &session,
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
            std::env::var_os("DISPLAY").is_some(),
        )
    } else {
        "native"
    };
    let wayland_hint = session_hint == "wayland";
    let capability = |id: &str, state, detail: &str| PlatformCapability {
        id: id.into(),
        state,
        detail: detail.into(),
    };
    PlatformReport {
        os: std::env::consts::OS.into(),
        session_hint: session_hint.into(),
        capabilities: vec![
            capability(
                "clipboard_capture",
                State::NotImplemented,
                "尚未接入系统剪贴板采集。",
            ),
            capability(
                "clipboard_write",
                State::NotImplemented,
                "尚未接入系统剪贴板写入。",
            ),
            capability(
                "global_shortcut",
                if wayland_hint {
                    State::Restricted
                } else {
                    State::NotImplemented
                },
                if wayland_hint {
                    "检测到 Wayland 会话线索；上游全局快捷键支持受限，尚未接入 portal。"
                } else {
                    "尚未注册全局快捷键；可用性需原生验证。"
                },
            ),
            capability(
                "paste_to_app",
                if wayland_hint {
                    State::Restricted
                } else {
                    State::NotImplemented
                },
                if wayland_hint {
                    "恢复外部焦点与模拟按键受 compositor 限制，尚未实现。"
                } else {
                    "尚未实现原应用回粘；权限与焦点恢复需原生验证。"
                },
            ),
            capability(
                "tray",
                State::NotImplemented,
                "尚未接入托盘；当前关闭窗口会退出应用。",
            ),
            capability(
                "sync",
                State::NotImplemented,
                "尚未实现云同步，不连接现有云端。",
            ),
        ],
    }
}

fn session_hint(session: &str, has_wayland: bool, has_x11: bool) -> &'static str {
    match session {
        "wayland" => "wayland",
        "x11" => "x11",
        _ if has_wayland => "wayland",
        _ if has_x11 => "x11",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_session_is_not_misreported_as_x11_when_xwayland_sets_display() {
        assert_eq!(session_hint("wayland", true, true), "wayland");
        assert_eq!(session_hint("", true, true), "wayland");
        assert_eq!(session_hint("x11", false, true), "x11");
        assert_eq!(session_hint("", false, false), "unknown");
    }
}
