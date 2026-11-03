use std::collections::HashMap;
use std::path::PathBuf;

use common::config::{Config, ConfigError};

fn base() -> HashMap<String, String> {
    [
        ("ADMIN_JWT_SECRET", "jwt-secret-value"),
        ("BASE_URL", "http://localhost:15633"),
        ("DISCORD_CLIENT_ID", "client-id"),
        ("DISCORD_CLIENT_SECRET", "client-secret-value"),
        ("DISCORD_TOKEN", "bot-token-value"),
        ("DISCORD_GUILD_ID", "guild-id"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn with(extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut m = base();
    for (k, v) in extra {
        m.insert((*k).to_string(), (*v).to_string());
    }
    m
}

#[test]
fn missing_jwt_secret_is_error() {
    let mut m = base();
    m.remove("ADMIN_JWT_SECRET");
    assert_eq!(
        Config::from_map(&m).unwrap_err(),
        ConfigError::Missing("ADMIN_JWT_SECRET")
    );
}

#[test]
fn every_required_variable_is_checked() {
    for name in [
        "ADMIN_JWT_SECRET",
        "BASE_URL",
        "DISCORD_CLIENT_ID",
        "DISCORD_CLIENT_SECRET",
        "DISCORD_TOKEN",
        "DISCORD_GUILD_ID",
    ] {
        let mut m = base();
        m.remove(name);
        assert_eq!(
            Config::from_map(&m).unwrap_err(),
            ConfigError::Missing(name)
        );
    }
}

#[test]
fn empty_required_value_is_missing() {
    let m = with(&[("DISCORD_TOKEN", ""), ("DISCORD_GUILD_ID", "")]);
    assert_eq!(
        Config::from_map(&m).unwrap_err(),
        ConfigError::Missing("DISCORD_TOKEN")
    );
    let m = with(&[("ADMIN_JWT_SECRET", "   ")]);
    assert_eq!(
        Config::from_map(&m).unwrap_err(),
        ConfigError::Missing("ADMIN_JWT_SECRET")
    );
}

#[test]
fn defaults_apply() {
    let cfg = Config::from_map(&base()).unwrap();
    assert_eq!(cfg.port, 15633);
    assert_eq!(cfg.legacy_url, "http://127.0.0.1:3001");
    assert_eq!(cfg.steam_bot_url, "http://127.0.0.1:15634");
    assert_eq!(cfg.static_dir, PathBuf::from("public"));
    assert!(cfg.cors_origins.is_empty());
    assert!(!cfg.production);
}

#[test]
fn overrides_apply_and_empty_optionals_fall_back() {
    let cfg = Config::from_map(&with(&[
        ("PORT", "8080"),
        ("LEGACY_URL", "http://legacy:1"),
        ("STEAM_BOT_URL", ""),
        ("STATIC_DIR", "dist"),
    ]))
    .unwrap();
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.legacy_url, "http://legacy:1");
    assert_eq!(cfg.steam_bot_url, "http://127.0.0.1:15634");
    assert_eq!(cfg.static_dir, PathBuf::from("dist"));
}

#[test]
fn invalid_port_is_error() {
    let err = Config::from_map(&with(&[("PORT", "abc")])).unwrap_err();
    assert!(matches!(err, ConfigError::Invalid("PORT", ref v) if v == "abc"));
    let err = Config::from_map(&with(&[("PORT", "70000")])).unwrap_err();
    assert!(matches!(err, ConfigError::Invalid("PORT", _)));
}

#[test]
fn production_true_when_node_env_production_or_https_base_url() {
    assert!(!Config::from_map(&base()).unwrap().production);
    assert!(
        Config::from_map(&with(&[("NODE_ENV", "production")]))
            .unwrap()
            .production
    );
    assert!(
        Config::from_map(&with(&[("BASE_URL", "https://machupeek.example")]))
            .unwrap()
            .production
    );
    assert!(
        !Config::from_map(&with(&[("NODE_ENV", "development")]))
            .unwrap()
            .production
    );
}

#[test]
fn cors_origins_split_and_trimmed() {
    let cfg = Config::from_map(&with(&[("CORS_ORIGINS", "https://a.com, https://b.com")])).unwrap();
    assert_eq!(cfg.cors_origins, vec!["https://a.com", "https://b.com"]);
    let cfg = Config::from_map(&with(&[("CORS_ORIGINS", " https://a.com ,, ")])).unwrap();
    assert_eq!(cfg.cors_origins, vec!["https://a.com"]);
}

#[test]
fn cors_falls_back_to_admin_allowed_origins() {
    let cfg = Config::from_map(&with(&[("ADMIN_ALLOWED_ORIGINS", "https://c.com")])).unwrap();
    assert_eq!(cfg.cors_origins, vec!["https://c.com"]);
    let cfg = Config::from_map(&with(&[
        ("CORS_ORIGINS", "https://a.com"),
        ("ADMIN_ALLOWED_ORIGINS", "https://c.com"),
    ]))
    .unwrap();
    assert_eq!(cfg.cors_origins, vec!["https://a.com"]);
}

#[test]
fn cors_defaults_to_base_url_origin_in_production() {
    let cfg = Config::from_map(&with(&[
        ("NODE_ENV", "production"),
        ("BASE_URL", "https://machupeek.example/some/path?x=1"),
    ]))
    .unwrap();
    assert_eq!(cfg.cors_origins, vec!["https://machupeek.example"]);

    let cfg = Config::from_map(&with(&[
        ("NODE_ENV", "production"),
        ("BASE_URL", "http://host.example:8080/"),
    ]))
    .unwrap();
    assert_eq!(cfg.cors_origins, vec!["http://host.example:8080"]);

    // Explicit list wins over the fallback.
    let cfg = Config::from_map(&with(&[
        ("NODE_ENV", "production"),
        ("CORS_ORIGINS", "https://a.com"),
    ]))
    .unwrap();
    assert_eq!(cfg.cors_origins, vec!["https://a.com"]);

    // Legacy only applies the fallback when NODE_ENV is production (not for https alone).
    let cfg = Config::from_map(&with(&[("BASE_URL", "https://machupeek.example")])).unwrap();
    assert!(cfg.cors_origins.is_empty());
}

#[test]
fn optional_server_fields_are_parsed() {
    let cfg = Config::from_map(&base()).unwrap();
    assert_eq!(cfg.cs2_server_public_ip, None);
    assert_eq!(cfg.mixton_server_ip, None);
    assert_eq!(cfg.admin_role_id, None);

    let cfg = Config::from_map(&with(&[
        ("CS2_SERVER_PUBLIC_IP", "1.2.3.4"),
        ("MIXTON_SERVER_IP", "5.6.7.8"),
        ("ADMIN_ROLE_ID", "999"),
    ]))
    .unwrap();
    assert_eq!(cfg.cs2_server_public_ip.as_deref(), Some("1.2.3.4"));
    assert_eq!(cfg.mixton_server_ip.as_deref(), Some("5.6.7.8"));
    assert_eq!(cfg.admin_role_id.as_deref(), Some("999"));

    let cfg = Config::from_map(&with(&[
        ("CS2_SERVER_PUBLIC_IP", ""),
        ("MIXTON_SERVER_IP", ""),
        ("ADMIN_ROLE_ID", ""),
    ]))
    .unwrap();
    assert_eq!(cfg.cs2_server_public_ip, None);
    assert_eq!(cfg.mixton_server_ip, None);
    assert_eq!(cfg.admin_role_id, None);
}

#[test]
fn debug_redacts_secrets() {
    let cfg = Config::from_map(&base()).unwrap();
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("jwt-secret-value"));
    assert!(!dbg.contains("client-secret-value"));
    assert!(!dbg.contains("bot-token-value"));
    assert!(dbg.contains("<redacted>"));
    assert!(dbg.contains("15633"));
}
