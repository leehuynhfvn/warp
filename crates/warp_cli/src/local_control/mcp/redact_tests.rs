use secret_redaction::regexes::DEFAULT_REGEXES_WITH_NAMES;

use super::*;

#[test]
fn every_pattern_compiles() {
    let redactor = Redactor::with_default_patterns();
    assert_eq!(
        redactor.patterns.len(),
        DEFAULT_REGEXES_WITH_NAMES.len() - NOT_SECRETS.len() + CONFIG_SECRETS.len()
    );
}

#[test]
fn an_aws_access_key_is_hidden() {
    let redactor = Redactor::with_default_patterns();
    let text = "key AKIAIOSFODNN7EXAMPLE here";
    assert_eq!(
        redactor.to_model_text(text),
        "key ******************** here"
    );
}

#[test]
fn only_the_value_of_a_secret_setting_is_hidden() {
    let redactor = Redactor::with_default_patterns();
    let text = "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n\
                db_password: \"hunter2\"\n\
                \"api_key\": \"abc123\",\n";
    assert_eq!(
        redactor.to_model_text(text),
        "AWS_SECRET_ACCESS_KEY=****************************************\n\
         db_password: \"*******\"\n\
         \"api_key\": \"******\",\n"
    );
}

#[test]
fn the_password_in_a_url_is_hidden() {
    let redactor = Redactor::with_default_patterns();
    assert_eq!(
        redactor.to_model_text("postgres://app:s3cret@db:5432/app"),
        "postgres://app:******@db:5432/app"
    );
}

#[test]
fn a_private_key_keeps_its_lines() {
    let redactor = Redactor::with_default_patterns();
    let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAK\n-----END RSA PRIVATE KEY-----\n";
    assert_eq!(
        redactor.to_model_text(text),
        "-----BEGIN RSA PRIVATE KEY-----\n******\n*****\n-----END RSA PRIVATE KEY-----\n"
    );
}

#[test]
fn addresses_and_ordinary_config_are_kept() {
    let redactor = Redactor::with_default_patterns();
    let text = "listen 10.0.0.5:443 ssl;\n\
                inet6 fe80::1/64 link/ether 52:54:00:12:34:56\n\
                PasswordAuthentication no\n\
                worker_connections 768;\n";
    assert!(matches!(redactor.to_model_text(text), Cow::Borrowed(_)));
    assert!(!redactor.contains_secrets(text));
}

#[test]
fn byte_offsets_survive_multibyte_secrets() {
    let redactor = Redactor::with_default_patterns();
    let text = "password=héllo\nnext";
    let redacted = redactor.to_model_text(text);
    assert_eq!(redacted.len(), text.len());
    assert_eq!(redacted, "password=******\nnext");
}

#[test]
fn disabled_keeps_everything() {
    let redactor = Redactor::disabled();
    let text = "AKIAIOSFODNN7EXAMPLE password=x";
    assert_eq!(redactor.to_model_text(text), text);
    assert!(!redactor.contains_secrets(text));
}
