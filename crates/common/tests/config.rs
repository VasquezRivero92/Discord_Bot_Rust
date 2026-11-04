use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use common::config::{firestore_mode, Config, ConfigError, EnvQueue, FirestoreMode};

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
fn jwt_secret_keeps_edge_whitespace() {
    let cfg = Config::from_map(&with(&[("ADMIN_JWT_SECRET", " s3cr3t ")])).unwrap();
    assert_eq!(cfg.admin_jwt_secret, " s3cr3t ");
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

#[test]
fn trust_cf_connecting_ip_defaults_to_false() {
    assert!(!Config::from_map(&base()).unwrap().trust_cf_connecting_ip);
    let m = with(&[("TRUST_CF_CONNECTING_IP", "")]);
    assert!(!Config::from_map(&m).unwrap().trust_cf_connecting_ip);
}

#[test]
fn trust_cf_connecting_ip_parses_values() {
    for v in ["1", "true", "TRUE", "Yes", "on"] {
        let m = with(&[("TRUST_CF_CONNECTING_IP", v)]);
        assert!(Config::from_map(&m).unwrap().trust_cf_connecting_ip, "{v}");
    }
    for v in ["0", "false", "No", "OFF"] {
        let m = with(&[("TRUST_CF_CONNECTING_IP", v)]);
        assert!(!Config::from_map(&m).unwrap().trust_cf_connecting_ip, "{v}");
    }
}

#[test]
fn trust_cf_connecting_ip_invalid_is_error() {
    let m = with(&[("TRUST_CF_CONNECTING_IP", "maybe")]);
    assert_eq!(
        Config::from_map(&m).unwrap_err(),
        ConfigError::Invalid("TRUST_CF_CONNECTING_IP", "maybe".into())
    );
}

#[test]
fn admin_role_name_is_the_fallback_for_admin_role_id() {
    // Como `env.ADMIN_ROLE_ID = ADMIN_ROLE_ID || ADMIN_ROLE_NAME` en config/index.js.
    let cfg = Config::from_map(&with(&[("ADMIN_ROLE_NAME", "STAFF")])).unwrap();
    assert_eq!(cfg.admin_role_id.as_deref(), Some("STAFF"));
    let cfg = Config::from_map(&with(&[
        ("ADMIN_ROLE_ID", ""),
        ("ADMIN_ROLE_NAME", "STAFF"),
    ]))
    .unwrap();
    assert_eq!(cfg.admin_role_id.as_deref(), Some("STAFF"));
    let cfg = Config::from_map(&with(&[
        ("ADMIN_ROLE_ID", "7"),
        ("ADMIN_ROLE_NAME", "STAFF"),
    ]))
    .unwrap();
    assert_eq!(cfg.admin_role_id.as_deref(), Some("7"));
}

// ------------------------------------------------------------ capa de entorno (getEffectiveConfig)

#[test]
fn env_defaults_read_the_legacy_variables() {
    let cfg = Config::from_map(&with(&[
        ("DISCORD_LINK_CHANNEL_ID", " 11 "),
        ("FACEIT_HUB_ID", "hub"),
        ("DISCORD_SUPPORT_CHANNEL_ID", ""),
        ("MATCH_TIMEOUT_MINUTES", "90"),
    ]))
    .unwrap();
    let env = &cfg.env_defaults;
    assert_eq!(env.discord_link_channel_id.as_deref(), Some("11"));
    assert_eq!(env.faceit_hub_id.as_deref(), Some("hub"));
    assert_eq!(env.discord_support_channel_id, None);
    assert_eq!(env.match_timeout_minutes.as_deref(), Some("90"));
    assert!(env.queues.is_empty());
}

#[test]
fn env_queues_numbered_like_get_queues_from_env() {
    let cfg = Config::from_map(&with(&[
        ("FACEIT_QUEUE1_ID", "q-a"),
        ("DISCORD_QUEUE1_NAME", "Alta"),
        ("DISCORD_QUEUE_1_MATCHES_CHANNEL_ID", "m1"),
        ("QUEUE1_HUB_ROLE_ID", "h1"),
        // la 2 solo tiene sala de espera: id por defecto y nombre por defecto
        ("DISCORD_QUEUE2_WAITING_ROOM_CHANNEL_ID", "w2"),
        // la 3 solo tiene mixton: no cuenta
        ("DISCORD_QUEUE3_MIXTON_CHANNEL_ID", "x3"),
        // las listas no se usan si hay colas numeradas
        ("DISCORD_MATCHES_CHANNEL_ID", "a,b"),
    ]))
    .unwrap();
    let q = &cfg.env_defaults.queues;
    assert_eq!(q.len(), 2, "{q:?}");
    assert_eq!(
        q[0],
        EnvQueue {
            id: "q-a".into(),
            queue_id: "q-a".into(),
            name: "Alta".into(),
            discord_matches_channel_id: "m1".into(),
            hub_role_id: Some("h1".into()),
            ..Default::default()
        }
    );
    assert_eq!(q[1].id, "queue_2");
    assert_eq!(q[1].name, "Cola 2");
    assert_eq!(q[1].discord_waiting_room_channel_id, "w2");
    assert_eq!(q[1].hub_role_id.as_deref(), Some(""));
}

#[test]
fn env_queues_from_comma_lists_need_more_than_one() {
    let cfg = Config::from_map(&with(&[("DISCORD_MATCHES_CHANNEL_ID", "solo")])).unwrap();
    assert!(cfg.env_defaults.queues.is_empty());

    let cfg = Config::from_map(&with(&[
        ("DISCORD_MATCHES_CHANNEL_ID", "m1, m2,,"),
        ("DISCORD_WAITING_ROOM_CHANNEL_ID", "w1"),
        ("FACEIT_QUEUE_IDS", "qa"),
        ("DISCORD_HUB_ROLE_ID", "dh"),
    ]))
    .unwrap();
    let q = &cfg.env_defaults.queues;
    assert_eq!(q.len(), 2);
    assert_eq!((q[0].id.as_str(), q[1].id.as_str()), ("qa", "queue_2"));
    assert_eq!(q[1].name, "Cola 2");
    assert_eq!(q[1].discord_matches_channel_id, "m2");
    // lista corta: cae al primero
    assert_eq!(q[1].discord_waiting_room_channel_id, "w1");
    assert_eq!(q[1].discord_mixton_channel_id, "");
    assert_eq!(q[1].hub_role_id, None);
    assert_eq!(q[1].discord_hub_role_id, "dh");
}

// ------------------------------------------------------------ BASE_URL

#[test]
fn base_url_trailing_slashes_are_stripped_once() {
    // config/index.js:28: `.replace(/\/+$/, '')`.
    let cfg = Config::from_map(&with(&[("BASE_URL", "https://machupeek.example///")])).unwrap();
    assert_eq!(cfg.base_url, "https://machupeek.example");
    let cfg = Config::from_map(&with(&[("BASE_URL", "https://machupeek.example/app/")])).unwrap();
    assert_eq!(cfg.base_url, "https://machupeek.example/app");
    let cfg = Config::from_map(&base()).unwrap();
    assert_eq!(cfg.base_url, "http://localhost:15633");
}

#[test]
fn base_url_made_only_of_slashes_is_invalid() {
    assert_eq!(
        Config::from_map(&with(&[("BASE_URL", "///")])).unwrap_err(),
        ConfigError::Invalid("BASE_URL", "///".into())
    );
}

// ------------------------------------------------------------ proxy XFF

#[test]
fn proxy_append_peer_to_xff_defaults_off_and_parses() {
    assert!(!Config::from_map(&base()).unwrap().proxy_append_peer_to_xff);
    let m = with(&[("PROXY_APPEND_PEER_TO_XFF", "1")]);
    assert!(Config::from_map(&m).unwrap().proxy_append_peer_to_xff);
    let m = with(&[("PROXY_APPEND_PEER_TO_XFF", "nope")]);
    assert_eq!(
        Config::from_map(&m).unwrap_err(),
        ConfigError::Invalid("PROXY_APPEND_PEER_TO_XFF", "nope".into())
    );
}

// ------------------------------------------------------------ proxy timeouts

#[test]
fn proxy_timeouts_are_disabled_when_unset_empty_or_zero() {
    let cfg = Config::from_map(&base()).unwrap();
    assert_eq!(cfg.proxy_first_byte_timeout, None);
    assert_eq!(cfg.ws_idle_timeout, None);
    for v in ["", "  ", "0", "00", " 0 "] {
        let cfg = Config::from_map(&with(&[
            ("PROXY_FIRST_BYTE_TIMEOUT_SECS", v),
            ("WS_IDLE_TIMEOUT_SECS", v),
        ]))
        .unwrap();
        assert_eq!(cfg.proxy_first_byte_timeout, None, "{v:?}");
        assert_eq!(cfg.ws_idle_timeout, None, "{v:?}");
    }
}

#[test]
fn proxy_timeouts_parse_whole_seconds() {
    let cfg = Config::from_map(&with(&[
        ("PROXY_FIRST_BYTE_TIMEOUT_SECS", "120"),
        ("WS_IDLE_TIMEOUT_SECS", " 900 "),
    ]))
    .unwrap();
    assert_eq!(cfg.proxy_first_byte_timeout, Some(Duration::from_secs(120)));
    assert_eq!(cfg.ws_idle_timeout, Some(Duration::from_secs(900)));
}

#[test]
fn invalid_proxy_timeouts_are_config_errors() {
    for key in ["PROXY_FIRST_BYTE_TIMEOUT_SECS", "WS_IDLE_TIMEOUT_SECS"] {
        for v in ["abc", "-1", "+5", "1.5", "30s", "99999999999999999999999"] {
            assert_eq!(
                Config::from_map(&with(&[(key, v)])).unwrap_err(),
                ConfigError::Invalid(key, v.into()),
                "{key}={v}"
            );
        }
    }
}

// ------------------------------------------------------------ Firestore

fn mode(extra: &[(&str, &str)]) -> Result<FirestoreMode, ConfigError> {
    firestore_mode(&Config::from_map(&with(extra)).unwrap())
}

#[test]
fn firestore_disabled_without_project_id() {
    assert_eq!(mode(&[]), Ok(FirestoreMode::Disabled));
    assert_eq!(
        mode(&[("FIRESTORE_PROJECT_ID", "  ")]),
        Ok(FirestoreMode::Disabled)
    );
    // Credential variables alone never enable Firestore.
    assert_eq!(
        mode(&[
            ("FIREBASE_SERVICE_ACCOUNT_PATH", "/k.json"),
            ("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080"),
            ("FIRESTORE_USE_ADC", "1"),
        ]),
        Ok(FirestoreMode::Disabled)
    );
}

#[test]
fn firestore_project_without_credentials_is_an_error_never_implicit_adc() {
    let err = mode(&[("FIRESTORE_PROJECT_ID", "p")]).unwrap_err();
    assert_eq!(err, ConfigError::NoFirestoreCredentials);
    let err = mode(&[("FIRESTORE_PROJECT_ID", "p"), ("FIRESTORE_USE_ADC", "0")]).unwrap_err();
    assert_eq!(err, ConfigError::NoFirestoreCredentials);
    let err = mode(&[
        ("FIRESTORE_PROJECT_ID", "p"),
        ("FIREBASE_SERVICE_ACCOUNT_PATH", ""),
        ("FIRESTORE_EMULATOR_HOST", " "),
    ])
    .unwrap_err();
    assert_eq!(err, ConfigError::NoFirestoreCredentials);
    // The message names every way out and no value.
    let msg = err.to_string();
    for name in [
        "FIRESTORE_EMULATOR_HOST",
        "FIREBASE_SERVICE_ACCOUNT_PATH",
        "FIRESTORE_USE_ADC",
    ] {
        assert!(msg.contains(name), "{msg}");
    }
    assert!(!msg.contains("  "), "{msg}");
}

#[test]
fn firestore_emulator_mode() {
    assert_eq!(
        mode(&[
            ("FIRESTORE_PROJECT_ID", "p"),
            ("FIRESTORE_EMULATOR_HOST", "127.0.0.1:8080")
        ]),
        Ok(FirestoreMode::Emulator {
            host: "127.0.0.1:8080".into()
        })
    );
}

#[test]
fn firestore_emulator_wins_over_key_file_and_adc() {
    // The crate talks to the emulator whenever the variable is set, so the
    // reported mode must say so (fail-safe: never production data).
    assert_eq!(
        mode(&[
            ("FIRESTORE_PROJECT_ID", "p"),
            ("FIRESTORE_EMULATOR_HOST", "localhost:8080"),
            ("FIREBASE_SERVICE_ACCOUNT_PATH", "/k.json"),
            ("FIRESTORE_USE_ADC", "1"),
        ]),
        Ok(FirestoreMode::Emulator {
            host: "localhost:8080".into()
        })
    );
}

#[test]
fn firestore_key_file_mode() {
    assert_eq!(
        mode(&[
            ("FIRESTORE_PROJECT_ID", "p"),
            ("FIREBASE_SERVICE_ACCOUNT_PATH", "/secrets/sa.json")
        ]),
        Ok(FirestoreMode::KeyFile {
            path: PathBuf::from("/secrets/sa.json")
        })
    );
}

#[test]
fn firestore_key_file_wins_over_adc() {
    assert_eq!(
        mode(&[
            ("FIRESTORE_PROJECT_ID", "p"),
            ("FIREBASE_SERVICE_ACCOUNT_PATH", "/k.json"),
            ("FIRESTORE_USE_ADC", "true"),
        ]),
        Ok(FirestoreMode::KeyFile {
            path: PathBuf::from("/k.json")
        })
    );
}

#[test]
fn firestore_adc_only_on_explicit_opt_in() {
    for v in ["1", "true", "YES", "on"] {
        assert_eq!(
            mode(&[("FIRESTORE_PROJECT_ID", "p"), ("FIRESTORE_USE_ADC", v)]),
            Ok(FirestoreMode::Adc),
            "{v}"
        );
    }
}

#[test]
fn firestore_use_adc_invalid_value_is_a_config_error() {
    let m = with(&[("FIRESTORE_USE_ADC", "si")]);
    assert_eq!(
        Config::from_map(&m).unwrap_err(),
        ConfigError::Invalid("FIRESTORE_USE_ADC", "si".into())
    );
}

#[test]
fn firestore_fields_are_parsed_and_debug_shows_no_secret() {
    let cfg = Config::from_map(&with(&[
        ("FIRESTORE_PROJECT_ID", "proj"),
        ("FIREBASE_SERVICE_ACCOUNT_PATH", "/k.json"),
    ]))
    .unwrap();
    assert_eq!(cfg.firestore_project_id.as_deref(), Some("proj"));
    assert_eq!(
        cfg.firebase_service_account_path,
        Some(PathBuf::from("/k.json"))
    );
    assert_eq!(cfg.firestore_emulator_host, None);
    assert!(!cfg.firestore_use_adc);
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("jwt-secret-value"));
}

#[test]
fn firestore_mode_describe_never_includes_secrets() {
    assert_eq!(FirestoreMode::Disabled.describe(), "disabled");
    assert_eq!(FirestoreMode::Adc.describe(), "adc");
    assert!(FirestoreMode::Emulator { host: "h:1".into() }
        .describe()
        .contains("h:1"));
    assert!(FirestoreMode::KeyFile {
        path: PathBuf::from("/k.json")
    }
    .describe()
    .starts_with("key-file"));
}

#[test]
fn aws_and_cs2_config_parsing_and_debug_redaction() {
    let cfg = Config::from_map(&with(&[
        ("AWS_REGION", "sa-east-1"),
        ("AWS_ACCESS_KEY_ID", "AKIA123"),
        ("AWS_SECRET_ACCESS_KEY", "super-secret-aws-key"),
        ("AWS_EC2_INSTANCE_ID", "i-1234567890abcdef0"),
        ("CS2_SERVER_IP", "10.0.0.1"),
        ("CS2_SSH_USER", "cs2user"),
        ("CS2_SSH_KEY_PATH", "/keys/test.pem"),
        ("CS2_SERVER_API_KEY", "cs2-secret-api-key"),
    ]))
    .unwrap();
    assert_eq!(cfg.aws_region, "sa-east-1");
    assert_eq!(cfg.aws_access_key_id.as_deref(), Some("AKIA123"));
    assert_eq!(cfg.aws_secret_access_key.as_deref(), Some("super-secret-aws-key"));
    assert_eq!(cfg.aws_ec2_instance_id.as_deref(), Some("i-1234567890abcdef0"));
    assert_eq!(cfg.cs2_server_ip, "10.0.0.1");
    assert_eq!(cfg.cs2_ssh_user, "cs2user");
    assert_eq!(cfg.cs2_ssh_key_path, Some(PathBuf::from("/keys/test.pem")));
    assert_eq!(cfg.cs2_server_api_key.as_deref(), Some("cs2-secret-api-key"));

    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("super-secret-aws-key"));
    assert!(!dbg.contains("cs2-secret-api-key"));
    assert!(dbg.contains("aws_secret_access_key: \"<redacted>\""));
    assert!(dbg.contains("cs2_server_api_key: \"<redacted>\""));
}
