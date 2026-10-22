//! The player's own configuration, on its own storage: for now the device id.

use std::path::PathBuf;

use library::DeviceId;

/// This device's id, kept next to the player's settings on its own storage
/// (`$DJPLAYER_CONFIG`, else `$XDG_CONFIG_HOME/djplayer`, else `~/.config/djplayer`), never
/// on a stick. Made on first use. Returns a warning if it could not be kept.
pub fn device_id() -> (DeviceId, Option<String>) {
    let dir = std::env::var_os("DJPLAYER_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(|d| PathBuf::from(d).join("djplayer")))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("djplayer"))
        });
    let Some(dir) = dir else {
        return (
            DeviceId::random(),
            Some("no config folder: a new device id for this run".into()),
        );
    };
    let path = dir.join("device-id");
    if let Some(id) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
    {
        return (id, None);
    }
    let id = DeviceId::random();
    let kept =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, format!("{id}\n")));
    let warning = kept.err().map(|e| {
        format!(
            "can't keep the device id in {} ({e}): a new one for this run",
            path.display()
        )
    });
    (id, warning)
}
