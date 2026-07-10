use rust_web_digest::{
    config::AppConfig,
    publication::{release_asset_name, repository_path, sha256_digest},
};

fn config() -> AppConfig {
    AppConfig::load("config/sources.toml").unwrap()
}

#[test]
fn phase7_uses_digest_release_naming() {
    let config = config();
    assert_eq!(config.newsletter.release_tag_prefix, "digest");
    assert_eq!(config.newsletter.release_name_prefix, "Rust Web Digest");
    assert_eq!(
        release_asset_name(&config.newsletter.release_asset_name_prefix, "2026-07"),
        "rust-web-digest-2026-07.md"
    );
}

#[test]
fn publication_path_must_be_repository_relative() {
    assert_eq!(
        repository_path("content/issues/2026-07.md").unwrap(),
        "content/issues/2026-07.md"
    );
    assert!(repository_path("../outside.md").is_err());
    assert!(repository_path("/tmp/absolute.md").is_err());
}

#[test]
fn newsletter_configuration_rejects_unsafe_asset_prefix() {
    let mut config = config();
    config.newsletter.release_asset_name_prefix = "rust web digest".to_owned();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("release_asset_name_prefix may contain only"));
}

#[test]
fn newsletter_configuration_rejects_empty_release_name_prefix() {
    let mut config = config();
    config.newsletter.release_name_prefix = "  ".to_owned();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("release_name_prefix cannot be empty"));
}

#[test]
fn publication_asset_digest_is_stable_sha256() {
    assert_eq!(
        sha256_digest(b"hello"),
        "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}
