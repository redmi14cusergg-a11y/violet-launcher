//! Офлайн-авторизація для Minecraft (режим без Microsoft/Mojang-акаунта).
//!
//! Додайте в Cargo.toml:
//!   md5  = "0.7"
//!   uuid = { version = "1", features = ["v3"] }
//!   serde = { version = "1", features = ["derive"] }  # вже є

use serde::{Deserialize, Serialize};
use std::process::Command;
use uuid::{Builder, Uuid};

/// Офлайн-UUID так само, як це робить Minecraft/Java:
/// `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))`.
///
/// УВАГА: це НЕ `Uuid::new_v3(namespace, ..)` — Java не додає namespace,
/// а просто бере MD5 від рядка й виставляє біти версії (3) та варіанту (RFC 4122).
pub fn offline_uuid(username: &str) -> Uuid {
    let digest = md5::compute(format!("OfflinePlayer:{username}").as_bytes());
    // from_md5_bytes виставляє version = 3 і variant = 10xx
    Builder::from_md5_bytes(digest.0).into_uuid()
}

/// Те саме, але рядком: `generate_offline_uuid("Notch")`.
/// Лаунчер передає UUID у грі зазвичай без дефісів — тому `.simple()`;
/// з дефісами — `.hyphenated()`.
pub fn generate_offline_uuid(username: &str) -> String {
    offline_uuid(username).hyphenated().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfflineSession {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UsernameError {
    Empty,
    TooLong,
    InvalidChar(char),
}

/// Java Edition: 3–16 символів `[A-Za-z0-9_]` (нікнейми коротші за 3 клієнт теж приймає,
/// тому мінімум перевіряємо лише на непорожність).
pub fn validate_username(name: &str) -> Result<(), UsernameError> {
    if name.is_empty() {
        return Err(UsernameError::Empty);
    }
    if name.chars().count() > 16 {
        return Err(UsernameError::TooLong);
    }
    if let Some(c) = name.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '_')) {
        return Err(UsernameError::InvalidChar(c));
    }
    Ok(())
}

impl OfflineSession {
    pub fn new(username: &str) -> Result<Self, UsernameError> {
        validate_username(username)?;
        let uuid = generate_offline_uuid(username);
        Ok(Self {
            username: username.to_owned(),
            // Токен для локальної гри нічого не значить; використовуємо "0"
            // або будь-який рядок. Зробимо детермінований: MD5 від UUID.
            access_token: format!("{:x}", md5::compute(uuid.as_bytes())),
            uuid,
            user_type: "legacy".into(),
        })
    }

    /// Аргументи гри (ті, що йдуть ПІСЛЯ main class).
    pub fn game_args(&self) -> Vec<String> {
        vec![
            "--username".into(), self.username.clone(),
            "--uuid".into(), self.uuid.clone(),
            "--accessToken".into(), self.access_token.clone(),
            "--userType".into(), self.user_type.clone(),
        ]
    }
}

/// Приклад складання команди запуску. `jvm_args`, `classpath`, `main_class`
/// та решту game-аргументів (--version, --gameDir, --assetsDir ...) ви вже маєте у лаунчері.
pub fn build_command(
    java: &str,
    jvm_args: &[String],
    classpath: &str,
    main_class: &str,
    session: &OfflineSession,
    extra_game_args: &[String],
) -> Command {
    let mut cmd = Command::new(java);
    cmd.args(jvm_args)
        .arg("-cp").arg(classpath)
        .arg(main_class)
        .args(session.game_args()) // кожен аргумент окремо — без shell, тож ніяких проблем з пробілами
        .args(extra_game_args);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // Відомі офлайн-UUID з реальних серверів (online-mode=false)
        assert_eq!(generate_offline_uuid("Notch"), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }

    #[test]
    fn version_and_variant() {
        let u = offline_uuid("Steve");
        assert_eq!(u.get_version_num(), 3);
        assert_eq!(u.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(u, offline_uuid("Steve")); // детермінований
        assert_ne!(u, offline_uuid("steve")); // регістр має значення
    }

    #[test]
    fn session_and_args() {
        let s = OfflineSession::new("Steve").unwrap();
        let a = s.game_args();
        assert_eq!(&a[0..2], ["--username", "Steve"]);
        assert_eq!(a[4], "--accessToken");
        assert_eq!(&a[6..8], ["--userType", "legacy"]);
        assert!(OfflineSession::new("bad name").is_err());
    }
}
