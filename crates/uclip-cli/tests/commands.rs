use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
};

const UCLIP: &str = env!("CARGO_BIN_EXE_uclip");

fn cli(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(UCLIP);
    command
        .args(["--data-dir"])
        .arg(directory)
        .args(args)
        .env("UCLIP_LANG", "en")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_SESSION_TYPE");
    command
}

fn run(directory: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = cli(directory, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if !input.is_empty() {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn successful(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn help_follows_locale_and_supports_explicit_overrides_without_opening_the_library() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("unused");
    for (override_locale, all, messages, lang, expected) in [
        (None, "", "zh_TW.UTF-8", "en_US", "\u{7528}\u{6cd5}"),
        (Some("system"), "en_US", "zh_CN", "zh_CN", "Usage:"),
        (Some("en"), "zh_CN", "", "", "Usage:"),
        (Some("zh-CN"), "C", "", "", "\u{7528}\u{6cd5}"),
        (None, "", "", "fr_FR", "Usage:"),
    ] {
        let mut command = cli(&library, &["--help"]);
        command
            .env_remove("UCLIP_LANG")
            .env("LC_ALL", all)
            .env("LC_MESSAGES", messages)
            .env("LANG", lang);
        if let Some(locale) = override_locale {
            command.env("UCLIP_LANG", locale);
        }
        let output = successful(command.output().unwrap());
        assert!(String::from_utf8(output).unwrap().contains(expected));
    }

    let output = successful(
        cli(&library, &["help", "sync", "remote"])
            .env("UCLIP_LANG", "zh-CN")
            .output()
            .unwrap(),
    );
    let help = String::from_utf8(output).unwrap();
    assert!(help.contains("WebDAV \u{5b57}\u{6bb5}"));
    assert!(help.contains("hostKey"));
    assert!(!help.contains("Print help") && !help.contains("Options:"));
    assert!(!library.exists());
}

#[test]
fn localized_diagnostics_keep_exit_codes_and_history_output() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("profile");
    let chinese = |args: &[&str]| {
        cli(&library, args)
            .env("UCLIP_LANG", "zh-CN")
            .output()
            .unwrap()
    };
    let missing = chinese(&["list"]);
    assert_eq!(missing.status.code(), Some(3));
    assert!(missing.stdout.is_empty());
    let diagnostic = String::from_utf8(missing.stderr).unwrap();
    assert!(diagnostic.starts_with("uclip: not_found: "));
    assert!(diagnostic.contains("\u{8bb0}\u{5f55}"));

    let invalid = chinese(&["list", "--json", "--full"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    let diagnostic = String::from_utf8(invalid.stderr).unwrap();
    assert!(diagnostic.starts_with("\u{9519}\u{8bef}"));
    assert!(diagnostic.contains("--json") && diagnostic.contains("--full"));

    let text = "original \u{7b14}\u{8bb0}";
    successful(run(&library, &["save"], text.as_bytes()));
    assert_eq!(
        successful(chinese(&["get", "latest", "--raw"])),
        text.as_bytes()
    );
    let arguments = ["list", "--json"];
    assert_eq!(
        successful(chinese(&arguments)),
        successful(run(&library, &arguments, b""))
    );
}

#[test]
fn commands_round_trip_text_images_and_references_in_a_private_library() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("profile");
    let empty = run(&library, &["list"], b"");
    assert_eq!(empty.status.code(), Some(3));
    assert!(!library.exists());

    let payload = "  \u{7b14}\u{8bb0}\nsecond line  \x1b";
    let id = String::from_utf8(successful(run(&library, &["save"], payload.as_bytes()))).unwrap();
    let id = id.trim();
    for selector in [
        vec!["get", id, "--raw"],
        vec!["get", "latest", "-r"],
        vec!["get", "--index", "0", "-r"],
    ] {
        assert_eq!(
            successful(run(&library, &selector, b"")),
            payload.as_bytes()
        );
    }
    assert_eq!(
        successful(run(&library, &["get", id], b"")),
        [payload.as_bytes(), b"\n"].concat()
    );
    let files = temp.path().join("\u{6587}\u{4ef6}.txt");
    std::fs::write(&files, b"file content").unwrap();
    let path = files.to_str().unwrap();
    let file_id = String::from_utf8(successful(run(
        &library,
        &["save", "--reference", path],
        b"",
    )))
    .unwrap();
    let reference = successful(run(&library, &["get", file_id.trim(), "--raw"], b""));
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&reference).unwrap(),
        vec![path]
    );

    let png = uclip_runtime::clipboard_native::png(1, 1, vec![22, 44, 66, 128]).unwrap();
    let image_id =
        String::from_utf8(successful(run(&library, &["save", "--image", "-"], &png))).unwrap();
    assert_eq!(
        successful(run(&library, &["get", image_id.trim(), "--raw"], b"")),
        png
    );

    let search = successful(run(&library, &["search", "SECOND", "--full"], b""));
    let record: serde_json::Value = serde_json::from_slice(&search).unwrap();
    assert_eq!(record["item"]["id"], id);
    assert_eq!(record["content"]["text"], payload);
    let syntax = run(&library, &["search", "[", "--regex"], b"");
    assert_eq!(syntax.status.code(), Some(2));
    assert!(syntax.stdout.is_empty());

    let missing = temp.path().join("missing");
    let failed = run(
        &library,
        &["save", "--file", missing.to_str().unwrap()],
        b"",
    );
    assert_eq!(failed.status.code(), Some(3));
    let all: serde_json::Value =
        serde_json::from_slice(&successful(run(&library, &["list", "--json"], b""))).unwrap();
    assert_eq!(all["total"], 3);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let collection = runtime.block_on(async {
        let core = uclip_core::CoreService::open(library.join("foundation.sqlite3"))
            .await
            .unwrap();
        let collection = core
            .create_collection(uclip_core::CreateCollection {
                name: "Notes".into(),
            })
            .await
            .unwrap();
        let policy = core.capture_policy().await.unwrap();
        core.update_capture_policy(uclip_core::UpdateCapturePolicy {
            enabled: false,
            exclude_pattern: "^excluded fixture$".into(),
            max_items: policy.max_items,
            expected_revision: policy.revision,
        })
        .await
        .unwrap();
        core.shutdown().await.unwrap();
        collection
    });
    let excluded = run(&library, &["save", "excluded fixture"], b"");
    assert_eq!(excluded.status.code(), Some(1));
    assert_eq!(excluded.stderr, b"uclip: excluded\n");
    successful(run(
        &library,
        &["save", "a classified note", "--collection", "Notes"],
        b"",
    ));
    for selector in [collection.name.as_str(), collection.id.as_str()] {
        assert_eq!(
            successful(run(
                &library,
                &["get", "latest", "--collection", selector, "--raw"],
                b""
            )),
            b"a classified note"
        );
    }

    for shell in ["bash", "zsh"] {
        let result = successful(run(&missing, &["completion", shell], b""));
        assert!(!result.is_empty());
        assert!(!missing.exists());
    }
}

#[test]
fn an_exact_collection_id_takes_precedence_over_an_older_collection_name() {
    use uclip_core::{CoreService, CreateCollection, RenameCollection};

    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("profile");
    successful(run(&library, &["save", "initial record"], b""));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let target = runtime.block_on(async {
        let core = CoreService::open(library.join("foundation.sqlite3"))
            .await
            .unwrap();
        let first = core
            .create_collection(CreateCollection {
                name: "First".into(),
            })
            .await
            .unwrap();
        let target = core
            .create_collection(CreateCollection {
                name: "Target".into(),
            })
            .await
            .unwrap();
        core.rename_collection(RenameCollection {
            id: first.id,
            name: target.id.clone(),
            expected_revision: first.revision,
        })
        .await
        .unwrap();
        core.shutdown().await.unwrap();
        target
    });
    successful(run(
        &library,
        &["save", "target record", "--collection", &target.id],
        b"",
    ));
    assert_eq!(
        successful(run(
            &library,
            &["get", "--collection", &target.name, "--raw"],
            b""
        )),
        b"target record"
    );
}

#[test]
fn full_history_uses_one_snapshot_while_the_output_pipe_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("profile");
    successful(run(&library, &["save", "first fixture"], b""));
    let large = "x".repeat(256 * 1024);
    successful(run(&library, &["save"], large.as_bytes()));

    let mut exporter = cli(&library, &["list", "--full"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The first record is larger than the pipe buffer and keeps the writer
    // blocked while another process changes the history revision.
    let mut first_byte = [0];
    exporter
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut first_byte)
        .unwrap();
    let concurrent_save = run(&library, &["save", "concurrent fixture"], b"");
    let output = successful(exporter.wait_with_output().unwrap());
    successful(concurrent_save);

    let output = [first_byte.as_slice(), &output].concat();
    let items = serde_json::Deserializer::from_slice(&output)
        .into_iter::<serde_json::Value>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["content"]["text"], large);
    assert_eq!(items[1]["content"]["text"], "first fixture");
}

#[test]
fn configures_remote_transport_offline_and_keeps_the_secret_out_of_status() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("profile");
    successful(run(&library, &["sync", "create"], b""));
    let config = br#"{"kind":"webdav","url":"https://uclip-fixture.invalid/","username":"fixture","password":"fixture-secret"}"#;
    let result = successful(run(&library, &["sync", "remote"], config));
    let result = std::str::from_utf8(&result).unwrap();
    assert!(result.contains("uclip-fixture.invalid"));
    assert!(!result.contains("fixture-secret"));
    let disabled = successful(run(&library, &["sync", "lan", "false"], b""));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&disabled).unwrap()["lanEnabled"],
        false
    );
    successful(run(&library, &["sync", "clear-remote"], b""));
    let refused = run(&library, &["serve"], b"");
    assert_eq!(refused.status.code(), Some(2));
    assert!(refused.stdout.is_empty());
}

#[cfg(unix)]
mod lan {
    use super::*;
    use std::{
        fs::File,
        process::Child,
        thread,
        time::{Duration, Instant},
    };

    struct Server(Child);

    impl Server {
        fn start(directory: &Path, log: &Path) -> Self {
            Self(
                cli(directory, &["serve"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(File::create(log).unwrap())
                    .spawn()
                    .unwrap(),
            )
        }

        fn stop(&mut self) {
            let signaled = Command::new("kill")
                .args(["-TERM", &self.0.id().to_string()])
                .status()
                .unwrap();
            assert!(signaled.success());
            wait_for(|| self.0.try_wait().unwrap().is_some());
            assert!(self.0.wait().unwrap().success());
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(25);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "the headless services did not converge"
            );
            thread::sleep(Duration::from_millis(200));
        }
    }

    #[test]
    #[ignore = "requires local IPv4 multicast"]
    fn headless_profiles_sync_lock_and_restart_without_a_display() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("one");
        let b = temp.path().join("two");
        successful(run(&a, &["sync", "create"], b""));
        let code = successful(run(&a, &["sync", "code"], b""));
        successful(run(&b, &["sync", "join"], &code));
        let first = successful(run(&a, &["save", "from A"], b""));
        let second = successful(run(&b, &["save", "from B"], b""));
        let a_id = std::str::from_utf8(&first).unwrap().trim();
        let b_id = std::str::from_utf8(&second).unwrap().trim();

        let a_log = temp.path().join("one.log");
        let b_log = temp.path().join("two.log");
        let mut server = Server::start(&a, &a_log);
        wait_for(|| std::fs::read_to_string(&a_log).is_ok_and(|log| log.contains("LAN Host")));
        let duplicate = run(&a, &["serve"], b"");
        assert_eq!(duplicate.status.code(), Some(4));
        let blocked = run(&a, &["sync", "lan", "false"], b"");
        assert_eq!(blocked.status.code(), Some(4));

        let mut peer = Server::start(&b, &b_log);
        wait_for(|| {
            run(&b, &["get", a_id, "--raw"], b"").stdout == b"from A"
                && run(&a, &["get", b_id, "--raw"], b"").stdout == b"from B"
        });
        assert!(!std::fs::read(&a_log)
            .unwrap()
            .windows(code.len() - 1)
            .any(|bytes| bytes == &code[..code.len() - 1]));
        server.stop();
        let mut server = Server::start(&a, &a_log);
        wait_for(|| {
            std::fs::read_to_string(&a_log)
                .is_ok_and(|log| log.contains("synchronization service started"))
        });
        assert_eq!(successful(run(&a, &["get", b_id, "--raw"], b"")), b"from B");
        server.stop();
        peer.stop();
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an isolated X11 display"]
fn x11_copy_outlives_the_cli_without_saving_history() {
    use uclip_core::ClipPayload;
    use uclip_runtime::clipboard::{self, Context, CLI_MARKER};
    use uclip_runtime::clipboard_x11::Reader;

    assert_eq!(std::env::var("UCLIP_FIXTURE_DISPLAY").as_deref(), Ok("1"));
    assert!(!matches!(
        std::env::var("DISPLAY").as_deref(),
        Err(_) | Ok(":0" | ":0.0")
    ));
    let directory = tempfile::tempdir().unwrap();
    let reader = Reader::new().unwrap();
    let result = Command::new(UCLIP)
        .args(["--data-dir"])
        .arg(directory.path())
        .args(["copy", "clipboard from a short-lived process"])
        .output()
        .unwrap();
    assert!(successful(result).is_empty());
    let formats = reader.formats().unwrap();
    assert!(formats.iter().any(|name| name == CLI_MARKER));
    assert_eq!(
        reader.payload(&formats).unwrap(),
        Some(ClipPayload::Text(
            "clipboard from a short-lived process".into()
        ))
    );
    assert!(!directory.path().join("foundation.sqlite3").exists());
    // Take ownership and release the short-lived CLI publisher.
    let context = Context::new().unwrap();
    clipboard::write(
        &context,
        ClipPayload::Text("replacement".into()),
        CLI_MARKER,
        b"test",
    )
    .unwrap();
}
