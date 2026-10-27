//! GTK4/Wayland has no primary-monitor API. On Plasma, read KScreen's output
//! priority asynchronously; never block the GTK thread on a desktop query.
use gtk4::{gdk, gio, prelude::*};
use std::cell::{Cell, RefCell};

thread_local! {
    static PRIMARY: RefCell<Option<String>> = const { RefCell::new(None) };
    static QUERYING: Cell<bool> = const { Cell::new(false) };
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn select_monitor(preferred: &str) -> Option<gdk::Monitor> {
    let monitors = gdk::Display::default()?.monitors();
    let connected: Vec<gdk::Monitor> = (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_downcast::<gdk::Monitor>())
        .filter(|m| m.is_valid() && m.geometry().height() > 0)
        .collect();
    let names: Vec<String> = connected
        .iter()
        .map(|m| m.connector().map(|s| s.to_string()).unwrap_or_default())
        .collect();
    let primary = PRIMARY.with(|p| p.borrow().clone());
    let index = select_index(&names, preferred, primary.as_deref())?;
    Some(connected[index].clone())
}

fn select_index(names: &[String], preferred: &str, primary: Option<&str>) -> Option<usize> {
    names
        .iter()
        .position(|n| !preferred.is_empty() && n == preferred)
        .or_else(|| primary.and_then(|p| names.iter().position(|n| n == p)))
        .or_else(|| (!names.is_empty()).then_some(0))
}

fn primary_from_json(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("outputs")?
        .as_array()?
        .iter()
        .filter(|o| o["connected"].as_bool() == Some(true) && o["enabled"].as_bool() == Some(true))
        .filter_map(|o| {
            let priority = o["priority"]
                .as_u64()
                .or_else(|| (o["primary"].as_bool() == Some(true)).then_some(1))?;
            (priority > 0).then(|| (priority, o["name"].as_str().map(str::to_owned)))
        })
        .min_by_key(|(priority, _)| *priority)
        .and_then(|(_, name)| name)
}

pub(super) fn with_primary(action: impl FnOnce() + 'static) {
    if INITIALIZED.with(Cell::get) {
        action();
    } else {
        PENDING.with(|p| p.borrow_mut().push(Box::new(action)));
        update_primary();
    }
}

fn finish_initial_query() {
    INITIALIZED.with(|i| i.set(true));
    let pending = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for action in pending {
        action();
    }
}

pub(super) fn update_primary() {
    if !std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .any(|d| d.eq_ignore_ascii_case("KDE"))
    {
        finish_initial_query();
        return;
    }
    if QUERYING.with(|q| q.replace(true)) {
        return;
    }
    let process = gio::Subprocess::newv(
        &[
            std::ffi::OsStr::new("kscreen-doctor"),
            std::ffi::OsStr::new("-j"),
        ],
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    );
    let Ok(process) = process else {
        QUERYING.with(|q| q.set(false));
        finish_initial_query();
        return;
    };
    let cancellable = gio::Cancellable::new();
    let timeout_cancel = cancellable.clone();
    let timeout_process = process.clone();
    let timeout = glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
        timeout_cancel.cancel();
        timeout_process.force_exit();
    });
    let callback_cancel = cancellable.clone();
    process.communicate_utf8_async(None, Some(&cancellable), move |result| {
        // The timeout removes itself when it fires.
        if !callback_cancel.is_cancelled() {
            timeout.remove();
        }
        QUERYING.with(|q| q.set(false));
        if let Ok((Some(stdout), _)) = result {
            let primary = primary_from_json(&stdout);
            let changed = PRIMARY.with(|p| {
                let mut old = p.borrow_mut();
                if *old == primary {
                    false
                } else {
                    *old = primary;
                    true
                }
            });
            if changed {
                super::imp::refresh_display();
            }
        }
        finish_initial_query();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_survives_disconnect_and_reconnect() {
        let both = vec!["HDMI-A-1".into(), "DP-1".into()];
        assert_eq!(select_index(&both, "", Some("DP-1")), Some(1));
        assert_eq!(select_index(&both, "HDMI-A-1", Some("DP-1")), Some(0));
        assert_eq!(
            select_index(&["DP-1".into()], "HDMI-A-1", Some("DP-1")),
            Some(0)
        );
        assert_eq!(select_index(&both, "HDMI-A-1", Some("DP-1")), Some(0));
        assert_eq!(select_index(&[], "HDMI-A-1", Some("DP-1")), None);
    }

    #[test]
    fn old_settings_follow_primary_and_preference_round_trips() {
        let old: crate::settings::QuickTerminalSettings =
            serde_json::from_str(r#"{"enabled":true,"height_fraction":0.45}"#).unwrap();
        assert!(old.preferred_display.is_empty());
        let configured: crate::settings::QuickTerminalSettings =
            serde_json::from_str(r#"{"preferred_display":"HDMI-A-1"}"#).unwrap();
        let saved = serde_json::to_string(&configured).unwrap();
        let restored: crate::settings::QuickTerminalSettings =
            serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.preferred_display, "HDMI-A-1");
    }

    #[test]
    fn primary_uses_priority_not_list_order_or_position() {
        let json = r#"{"outputs":[
            {"name":"HDMI-A-1","connected":true,"enabled":true,"priority":2,"pos":{"y":0}},
            {"name":"DP-1","connected":true,"enabled":true,"priority":1,"pos":{"y":2160}},
            {"name":"eDP-1","connected":false,"enabled":false,"priority":0}
        ]}"#;
        assert_eq!(primary_from_json(json).as_deref(), Some("DP-1"));
        assert_eq!(primary_from_json("{}"), None);
    }
}
