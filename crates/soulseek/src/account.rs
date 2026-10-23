//! The SoulSeek account, kept on the player's own storage (its config folder), never on a
//! stick: a stick goes from player to player, and the password with it would too.

use std::io::Write;
use std::path::Path;

use super::Credentials;

/// The account's file in the player's config folder: the user name, then the password, a
/// line each. Readable by its owner only.
pub const ACCOUNT_FILE: &str = "soulseek-account";

impl Credentials {
    /// The account kept in `config` ([`ACCOUNT_FILE`]), else the one from the environment
    /// ([`Credentials::from_env`]).
    pub fn load(config: Option<&Path>) -> Option<Self> {
        config
            .and_then(|d| std::fs::read_to_string(d.join(ACCOUNT_FILE)).ok())
            .and_then(|s| parse(&s))
            .or_else(Self::from_env)
    }

    /// Keep `account` in `config` (`None` removes it).
    pub fn save(account: Option<&Self>, config: &Path) -> std::io::Result<()> {
        let path = config.join(ACCOUNT_FILE);
        let Some(a) = account else {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        };
        std::fs::create_dir_all(config)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(&path)?;
        // `mode` only applies to a new file.
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        writeln!(f, "{}\n{}", a.user, a.password)
    }
}

/// A user name and password, a line each; both needed.
fn parse(s: &str) -> Option<Credentials> {
    let mut lines = s.lines();
    let user = lines.next()?.trim().to_string();
    let password = lines.next()?.to_string();
    (!user.is_empty() && !password.is_empty()).then_some(Credentials { user, password })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_kept_read_back_and_removed() {
        let dir = std::env::temp_dir().join(format!("soulseek-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Credentials {
            user: "dj".into(),
            password: "pass word ".into(),
        };
        Credentials::save(Some(&a), &dir).unwrap();
        assert_eq!(
            parse(&std::fs::read_to_string(dir.join(ACCOUNT_FILE)).unwrap()),
            Some(a.clone()),
            "spaces in the password are kept"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(ACCOUNT_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only its owner reads it");
        }
        Credentials::save(None, &dir).unwrap();
        Credentials::save(None, &dir).unwrap();
        assert!(!dir.join(ACCOUNT_FILE).exists());
        assert_eq!(parse("dj\n"), None, "both lines are needed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
