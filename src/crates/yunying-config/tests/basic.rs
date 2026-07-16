//! yunying-config 单元测试：默认值、JSON 往返、env 覆盖、脱敏、加载优先级。

use yunying_config::Config;

#[test]
fn defaults_match_operations_doc() {
    let c = Config::default();
    assert_eq!(c.goose.provider, "openai");
    assert_eq!(c.goose.model, "gpt-5.5");
    assert_eq!(c.goose.history_message_limit, 40); // BEAV_GOOSE_HISTORY_MESSAGE_LIMIT
    assert_eq!(c.mediacrawler.default_login_type, "qrcode");
    assert_eq!(c.mediacrawler.max_notes_count, 50);
    assert_eq!(c.video.model, "doubao-seedance-1.5-pro");
    assert_eq!(
        c.video.endpoint,
        "https://ark.cn-beijing.volces.com/api/plan/v3"
    );
    assert_eq!(c.server.bridge_port, 0);
    assert!(!c.safety.run_real_publish); // 默认不真实发布
}

#[test]
fn json_roundtrip() {
    let c = Config::default();
    let s = serde_json::to_string(&c).unwrap();
    let back: Config = serde_json::from_str(&s).unwrap();
    assert_eq!(back.goose.model, c.goose.model);
}

#[test]
fn partial_json_uses_defaults() {
    // 只给 goose 段，其余走默认（#[serde(default)]）。
    let raw = r#"{"goose":{"model":"gpt-5.5","base_url":"https://x"}}{"#;
    // 上面的 JSON 故意带多余字符以触发错误分支
    assert!(Config::from_json(raw).is_err());

    let raw = r#"{"goose":{"model":"custom-model"}}"#;
    let c = Config::from_json(raw).unwrap();
    assert_eq!(c.goose.model, "custom-model");
    // 未指定项保持默认
    assert_eq!(c.goose.provider, "openai");
    assert_eq!(c.video.resolution, "720p");
}

#[test]
fn redact_masks_sensitive() {
    let mut c = Config::default();
    c.goose.api_key = "sk-secret-123".into();
    c.mediacrawler.xhs_cookies = "web_session=abc".into();
    let r = c.redact();
    assert_eq!(r.goose.api_key, "[redacted:13 chars]"); // "sk-secret-123" = 13
    assert_eq!(r.mediacrawler.xhs_cookies, "[redacted:15 chars]"); // "web_session=abc" = 15
                                                                   // 原对象未变
    assert_eq!(c.goose.api_key, "sk-secret-123");
}

#[test]
fn empty_secret_stays_empty_after_redact() {
    let c = Config::default();
    let r = c.redact();
    assert_eq!(r.goose.api_key, "");
}

#[test]
fn env_overrides_applied() {
    std::env::set_var("OPENAI_API_KEY", "sk-from-env");
    std::env::set_var("RUN_REAL_CRAWLER", "1");
    let mut c = Config::default();
    yunying_config::loader::apply_env_overrides(&mut c);
    assert_eq!(c.goose.api_key, "sk-from-env");
    assert!(c.safety.run_real_crawler);
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("RUN_REAL_CRAWLER");
}

#[test]
fn resolve_path_prefers_explicit() {
    let p = yunying_config::loader::resolve_path(Some(std::path::Path::new("/tmp/x.json")));
    assert_eq!(p, Some(std::path::PathBuf::from("/tmp/x.json")));
}

#[test]
fn load_none_uses_env_config_from_arbitrary_cwd() {
    const CHILD_MARKER: &str = "YUNYING_CONFIG_ARBITRARY_CWD_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let loaded = yunying_config::load(None).unwrap();
        assert_eq!(loaded.goose.model, "model-from-app-data");
        return;
    }

    let app_data = tempfile::tempdir().unwrap();
    let unrelated_cwd = tempfile::tempdir().unwrap();
    let config_path = app_data.path().join("config.json");
    std::fs::write(&config_path, r#"{"goose":{"model":"model-from-app-data"}}"#).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("load_none_uses_env_config_from_arbitrary_cwd")
        .arg("--nocapture")
        .current_dir(unrelated_cwd.path())
        .env(CHILD_MARKER, "1")
        .env("YUNYING_CONFIG_PATH", &config_path)
        .env_remove("GOOSE_MODEL")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child config test failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
