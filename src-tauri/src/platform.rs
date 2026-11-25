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
                if wayland_hint {
                    State::Restricted
                } else {
                    State::Unverified
                },
                "已实现文本、图片、文件引用适配；运行状态见采集设置，目标平台需原生验收。",
            ),
            capability(
                "clipboard_write",
                if wayland_hint {
                    State::Restricted
                } else {
                    State::Unverified
                },
                "已实现系统写回；完成 OS 写入才报告成功，文件失效整批拒绝。",
            ),
            capability(
                "global_shortcut",
                if wayland_hint {
                    State::Restricted
                } else {
                    State::Unverified
                },
                if wayland_hint {
                    "检测到 Wayland 会话线索；上游全局快捷键支持受限，尚未接入 portal。"
                } else {
                    "默认 Ctrl/Cmd+Shift+V，可在桌面设置修改；冲突保持原设置，实际注册状态见桌面设置。"
                },
            ),
            capability(
                "paste_to_app",
                if wayland_hint {
                    State::Restricted
                } else if cfg!(any(target_os = "linux", target_os = "windows")) {
                    State::Unverified
                } else {
                    State::NotImplemented
                },
                if wayland_hint {
                    "恢复外部焦点与模拟按键受 compositor 限制，尚未实现。"
                } else {
                    "Windows 和 Linux X11 在验证原应用焦点后请求回粘。"
                },
            ),
            capability(
                "tray",
                State::Unverified,
                "托盘可打开应用菜单。Linux 根据桌面宿主选择自定义浮窗或原生菜单。",
            ),
            capability(
                "sync",
                State::Unverified,
                "同步设置显示局域网发现、WebDAV 和 SFTP 的当前状态。",
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
