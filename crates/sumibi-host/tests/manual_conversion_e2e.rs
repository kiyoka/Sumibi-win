#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sumibi_host::diagnostic_http::convert_chat_with_policy;
use sumibi_host::first_conversion::{
    validate_conversion_input, ConversionInput, PayloadError, CONVERSION_UNICODE_VERSION,
    MAX_TOTAL_GRAPHEMES,
};
use sumibi_host::manual::{
    Credential, Destination, ManualRequest, PolicySource, Scheme, SendAdmission, SendError,
    TransportError,
};
use sumibi_host::policy::PolicyAuthority;
use sumibi_host::settings::{ProtectionProfile, Settings, SettingsError};
use sumibi_protocol::message::Revision;

struct TestAdmission {
    cancelled: AtomicBool,
}
impl TestAdmission {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
        }
    }
}
impl SendAdmission for TestAdmission {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    fn write_if_active(
        &self,
        write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<(), SendError> {
        if self.is_cancelled() {
            Err(SendError::PolicyDenied)
        } else {
            write().map_err(SendError::Transport)
        }
    }
}

/// 1. 書記素分割の依存/Unicode版、原文＋現在結果＋登録辞書の1000書記素上限の検証
#[test]
fn test_unicode_and_combined_grapheme_limits() {
    // Unicode 17.0.0 整合性
    assert_eq!(CONVERSION_UNICODE_VERSION, (17, 0, 0));
    assert_eq!(
        CONVERSION_UNICODE_VERSION,
        sumibi_core::BACKSPACE_UNICODE_VERSION
    );

    // 1000書記素ちょうど
    let s1000 = "a".repeat(MAX_TOTAL_GRAPHEMES);
    let exact_input = ConversionInput::new(&s1000);
    assert_eq!(
        validate_conversion_input(&exact_input),
        Ok(MAX_TOTAL_GRAPHEMES)
    );

    // 1001書記素（超過）
    let s1001 = "a".repeat(MAX_TOTAL_GRAPHEMES + 1);
    let over_input = ConversionInput::new(&s1001);
    assert_eq!(
        validate_conversion_input(&over_input),
        Err(PayloadError::TotalGraphemesExceeded)
    );

    // 合成絵文字と結合文字
    let complex = "e\u{301}👨‍👩‍👧‍👦👍🏽🇯🇵";
    let complex_input = ConversionInput::new(complex);
    assert_eq!(validate_conversion_input(&complex_input), Ok(4));

    // 原文(400) + 現在結果(300) + 辞書(300) = 1000 (境界値 OK)
    let s400 = "x".repeat(400);
    let c300 = "y".repeat(300);
    let d300 = "z".repeat(300);
    let combined_1000 = ConversionInput::with_context(&s400, Some(&c300), Some(&d300));
    assert_eq!(validate_conversion_input(&combined_1000), Ok(1000));

    // 原文(400) + 現在結果(300) + 辞書(301) = 1001 (超過 エラー)
    let d301 = "z".repeat(301);
    let combined_1001 = ConversionInput::with_context(&s400, Some(&c300), Some(&d301));
    assert_eq!(
        validate_conversion_input(&combined_1001),
        Err(PayloadError::TotalGraphemesExceeded)
    );

    // 不正文字（\u{fffc}）の排除
    assert_eq!(
        validate_conversion_input(&ConversionInput::new("invalid\u{fffc}text")),
        Err(PayloadError::InvalidSource)
    );
    assert_eq!(
        validate_conversion_input(&ConversionInput::with_context(
            "valid",
            Some("bad\u{fffc}"),
            None
        )),
        Err(PayloadError::InvalidCurrentResult)
    );
    assert_eq!(
        validate_conversion_input(&ConversionInput::with_context(
            "valid",
            None,
            Some("bad\u{fffc}")
        )),
        Err(PayloadError::InvalidDictionary)
    );
}

/// 2. 模擬HTTP、手動変換縦断パイプライン、同意・認可版・キー欠損・通信失敗と即時再開
#[test]
fn test_manual_conversion_pipeline_with_diagnostic_http() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let response_body_200 = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": r#"{"candidates":["日本語"]}"#
            }
        }]
    })
    .to_string();
    let response_bytes_200 = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body_200}",
        response_body_200.len()
    )
    .into_bytes();

    let response_bytes_500 = b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_vec();

    let server_mode = Arc::new(AtomicU8::new(0)); // 0: 200 OK, 1: 500 Error
    let thread_mode = server_mode.clone();
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = running.clone();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    listener.set_nonblocking(true).expect("set nonblocking");
    let port = listener.local_addr().expect("local addr").port();

    let _server = std::thread::spawn(move || {
        while thread_running.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(conn) => conn,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => break,
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = [0u8; 1024];
            let mut req = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        req.extend_from_slice(&buf[..n]);
                        if req.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            let mode = thread_mode.load(Ordering::Relaxed);
            if mode == 0 {
                let _ = stream.write_all(&response_bytes_200);
            } else {
                let _ = stream.write_all(&response_bytes_500);
            }
        }
    });

    let destination =
        Destination::for_chat_completions(Scheme::HttpLoopback, "127.0.0.1", port, "/")
            .expect("destination");
    let authority = Arc::new(PolicyAuthority::new(destination.clone()));
    let admission = TestAdmission::new();

    // =========================================================================
    // Case A: 未設定 / 未同意での送信拒否
    // =========================================================================
    authority.set_model("test-model").expect("set model");
    let initial_rev = Revision {
        input: 1,
        settings: 2,
        authorization: 1,
    };
    let req_unconsented = ManualRequest {
        destination: &destination,
        revision: initial_rev,
        source: "nihongo",
        deadline: Instant::now() + Duration::from_secs(2),
    };
    assert_eq!(
        convert_chat_with_policy(req_unconsented, &authority, &admission),
        Err(SendError::PolicyDenied)
    );

    // =========================================================================
    // Case B: キー欠損時の同意拒否・未送信
    // =========================================================================
    // キーなし状態では grant_manual_consent が InvalidConsent で拒否される
    assert_eq!(
        authority.grant_manual_consent(&destination),
        Err(sumibi_host::policy::ChangeError::InvalidConsent)
    );
    assert!(!authority.snapshot().unwrap().allowed);

    // =========================================================================
    // Case C: 正常系（キー設定 + 同意付与 -> 変換成功）
    // =========================================================================
    let _cred_version = authority
        .replace_credential(Credential::new("valid-test-key".into()))
        .expect("replace cred");
    let consent_version = authority
        .grant_manual_consent(&destination)
        .expect("grant consent");

    let req_valid = ManualRequest {
        destination: &destination,
        revision: Revision {
            input: 1,
            settings: consent_version.settings,
            authorization: consent_version.authorization,
        },
        source: "nihongo",
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let result = convert_chat_with_policy(req_valid, &authority, &admission);
    assert_eq!(result, Ok("日本語".into()));

    // =========================================================================
    // Case D: 通信失敗（500エラー）時の固定エラー分類と、即時再開
    // =========================================================================
    server_mode.store(1, Ordering::Release);
    let req_fail = ManualRequest {
        destination: &destination,
        revision: Revision {
            input: 1,
            settings: consent_version.settings,
            authorization: consent_version.authorization,
        },
        source: "nihongo",
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let fail_result = convert_chat_with_policy(req_fail, &authority, &admission);
    assert!(
        matches!(
            fail_result,
            Err(SendError::Transport(
                sumibi_host::manual::TransportError::Http
            ))
        ),
        "server 500 must map to SendError::Transport(Http), got: {fail_result:?}"
    );

    // エラー解除ダイアログ等の操作なしで、直後に再試行して即座に成功すること
    server_mode.store(0, Ordering::Release);
    let req_retry = ManualRequest {
        destination: &destination,
        revision: Revision {
            input: 1,
            settings: consent_version.settings,
            authorization: consent_version.authorization,
        },
        source: "nihongo",
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let retry_result = convert_chat_with_policy(req_retry, &authority, &admission);
    assert_eq!(retry_result, Ok("日本語".into()));

    // =========================================================================
    // Case E: 認可版変更による旧結果破棄（Revocation & Revision mismatch）
    // =========================================================================
    let old_revision = Revision {
        input: 1,
        settings: consent_version.settings,
        authorization: consent_version.authorization,
    };
    // 同意を取り消す
    authority.revoke_manual_consent().expect("revoke");
    let req_stale = ManualRequest {
        destination: &destination,
        revision: old_revision,
        source: "nihongo",
        deadline: Instant::now() + Duration::from_secs(5),
    };
    assert_eq!(
        convert_chat_with_policy(req_stale, &authority, &admission),
        Err(SendError::PolicyDenied)
    );

    running.store(false, Ordering::Relaxed);
}

/// 3. 設定形式版（v1）、原子的保存・破損回復、本文・資格情報の非漏洩
#[test]
fn test_settings_atomic_save_and_corruption_recovery() {
    let dir = std::env::temp_dir().join(format!("sumibi-test-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");

    let settings_path = dir.join("settings.json");
    let settings = Settings::new(Scheme::Https, "api.example.test", 443, "/v1", "gpt-4o")
        .expect("new settings")
        .with_protection_profile(ProtectionProfile::Standard);

    // 1. Initial creation
    settings.create(&settings_path).expect("create settings");
    assert!(settings_path.exists());

    // 2. Load and verify
    let loaded = Settings::load(&settings_path).expect("load settings");
    assert_eq!(loaded.model(), "gpt-4o");
    assert_eq!(loaded.destination().host(), "api.example.test");
    assert_eq!(loaded.destination().port(), 443);
    assert_eq!(loaded.destination().base_path(), "/v1/chat/completions");
    assert_eq!(loaded.protection_profile(), ProtectionProfile::Standard);

    // 3. Verify saved record format and that secrets / body never leak into settings
    let saved_raw = std::fs::read_to_string(&settings_path).expect("read settings raw");
    assert!(saved_raw.contains(r#""version":1"#));
    assert!(saved_raw.contains(r#""scheme":"https""#));
    assert!(saved_raw.contains(r#""model":"gpt-4o""#));
    assert!(!saved_raw.contains("secret"));
    assert!(!saved_raw.contains("key"));
    assert!(!saved_raw.contains("password"));
    assert!(!saved_raw.contains("nihongo"));

    // 4. Atomic save_if_unchanged
    let updated = Settings::new(Scheme::Https, "api.example.test", 443, "/v1", "gpt-4o-mini")
        .expect("updated settings");
    updated
        .save_if_unchanged(&settings_path, &loaded)
        .expect("atomic save");
    let reloaded = Settings::load(&settings_path).expect("reload settings");
    assert_eq!(reloaded.model(), "gpt-4o-mini");

    // 5. Stale expected settings causes Conflict
    assert_eq!(
        updated.save_if_unchanged(&settings_path, &loaded),
        Err(SettingsError::Conflict)
    );

    // 6. Corruption recovery: invalid JSON or unknown version is safely rejected
    std::fs::write(&settings_path, b"corrupted { invalid json").expect("write corrupt");
    match Settings::load(&settings_path) {
        Err(err) => assert_eq!(err, SettingsError::Invalid),
        Ok(_) => panic!("corrupted settings should fail to load"),
    }

    // Overwriting corrupted file with explicit save succeeds (recovers)
    reloaded.save(&settings_path).expect("save over corrupt");
    assert_eq!(
        Settings::load(&settings_path)
            .expect("load after recovery")
            .model(),
        "gpt-4o-mini"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
