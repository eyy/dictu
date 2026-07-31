//! the global shortcut, as GNOME stores it.
//!
//! the key that looks a word up from anywhere is not ours: it is a GNOME *custom
//! keybinding*, three strings in dconf that the shell reads. so this module does not
//! grab a key — it reads and writes the reader's own configuration, which is why the
//! window can show what the shortcut currently is and change it (#67).
//!
//! the layout, for anyone who has not met it: one schema holds a list of paths,
//!
//! ```text
//! org.gnome.settings-daemon.plugins.media-keys custom-keybindings
//!   → ['…/custom0/', '…/custom1/']
//! ```
//!
//! and each path carries `name`, `command` and `binding` under the relocatable schema
//! `org.gnome.settings-daemon.plugins.media-keys.custom-keybinding`. the list is shared
//! with every other application that has ever added a shortcut, so ours is found by its
//! command and a new one is *appended* — never written over somebody else's row.

use anyhow::{Context, Result, bail};
use gtk::gio;
use gtk::prelude::*;

const KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const BINDING_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const BINDING_ROOT: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings";
/// how ours is recognised among everyone else's rows.
const MARKER: &str = "dictu";
/// what a fresh binding is called in the Settings app's own list.
const NAME: &str = "dictu: look up the selected word";

/// the shortcut as it stands: the accelerator GNOME holds, and the command it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub accelerator: String,
    pub command: String,
}

/// whether this desktop stores shortcuts the way we know how to read. false on a
/// machine without the GNOME settings daemon's schemas, where the window should say so
/// rather than offer a control that cannot work.
pub fn available() -> bool {
    schema(KEYS_SCHEMA) && schema(BINDING_SCHEMA)
}

fn schema(id: &str) -> bool {
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(id, true))
        .is_some()
}

/// the shortcut that runs dictu, if the reader has one.
pub fn current() -> Option<Shortcut> {
    if !available() {
        return None;
    }
    let keys = gio::Settings::new(KEYS_SCHEMA);
    for path in keys.strv("custom-keybindings") {
        let entry = gio::Settings::with_path(BINDING_SCHEMA, &path);
        let command = entry.string("command").to_string();
        if command.contains(MARKER) {
            return Some(Shortcut {
                accelerator: entry.string("binding").to_string(),
                command,
            });
        }
    }
    None
}

/// point the shortcut at `accelerator`, creating the binding if there is none.
///
/// `command` is only used when creating one; an existing row keeps whatever command it
/// already runs, because that script is the reader's and may well have been edited.
pub fn set(accelerator: &str, command: &str) -> Result<()> {
    if !available() {
        bail!("this desktop does not store shortcuts in GNOME's media-keys schema");
    }
    let keys = gio::Settings::new(KEYS_SCHEMA);
    let paths: Vec<String> = keys
        .strv("custom-keybindings")
        .iter()
        .map(|p| p.to_string())
        .collect();

    for path in &paths {
        let entry = gio::Settings::with_path(BINDING_SCHEMA, path);
        if entry.string("command").contains(MARKER) {
            entry
                .set_string("binding", accelerator)
                .context("writing the accelerator")?;
            return Ok(());
        }
    }

    // none yet: take the first unused `customN` and append it. appending matters — the
    // list belongs to every application that has ever added a shortcut.
    let path = (0..)
        .map(|n| format!("{BINDING_ROOT}/custom{n}/"))
        .find(|candidate| !paths.contains(candidate))
        .context("no free custom-keybinding slot")?;

    let entry = gio::Settings::with_path(BINDING_SCHEMA, &path);
    entry.set_string("name", NAME).context("naming it")?;
    entry
        .set_string("command", command)
        .context("setting its command")?;
    entry
        .set_string("binding", accelerator)
        .context("writing the accelerator")?;

    let mut all = paths;
    all.push(path);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    keys.set_strv("custom-keybindings", refs)
        .context("adding it to the list")?;
    Ok(())
}

/// is this accelerator one a reader could not undo?
///
/// a bare letter would swallow that letter everywhere on the desktop, and GNOME accepts
/// it without complaint — so it is refused here, where there is somebody to tell. the
/// rule: something must hold it down, unless it is a function key, which nothing else
/// wants.
///
/// parsed here rather than by `gtk::accelerator_parse`, which needs gtk initialised and
/// would make this a test that only runs with a display.
pub fn sensible(accelerator: &str) -> bool {
    let (modifiers, key) = split(accelerator);
    if key.is_empty() {
        return false;
    }
    let held = modifiers.iter().any(|m| {
        matches!(
            m.to_ascii_lowercase().as_str(),
            "control" | "ctrl" | "primary" | "alt" | "super" | "meta" | "hyper"
        )
    });
    // shift alone is not holding it down: <Shift>d is still a letter you have to type.
    held || function_key(&key)
}

/// `<Control><Alt>d` → (["Control", "Alt"], "d"). anything malformed comes back with an
/// empty key, which `sensible` refuses.
fn split(accelerator: &str) -> (Vec<String>, String) {
    let mut modifiers = Vec::new();
    let mut rest = accelerator;
    while let Some(stripped) = rest.strip_prefix('<') {
        match stripped.split_once('>') {
            Some((name, tail)) => {
                modifiers.push(name.to_string());
                rest = tail;
            }
            None => return (modifiers, String::new()),
        }
    }
    (modifiers, rest.to_string())
}

/// F1 to F35 — the keys nothing else has a use for, so they may stand on their own.
fn function_key(key: &str) -> bool {
    let Some(number) = key.strip_prefix(['F', 'f']) else {
        return false;
    };
    number.parse::<u8>().is_ok_and(|n| (1..=35).contains(&n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_needs_a_modifier_unless_it_is_a_function_key() {
        for good in ["<Super>F2", "<Control><Alt>d", "<Super>backslash", "F9"] {
            assert!(sensible(good), "{good} should be allowed");
        }
        // a bare letter would eat typing everywhere; a lone modifier is not a shortcut;
        // and nonsense should not reach dconf at all.
        for bad in ["d", "backslash", "<Super>", "", "nonsense"] {
            assert!(!sensible(bad), "{bad:?} should be refused");
        }
    }
}
