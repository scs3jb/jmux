//! Global hotkey for the quick terminal via `org.freedesktop.portal.
//! GlobalShortcuts` (feature `quick-terminal`).
//!
//! This mirrors ghostty's approach (`apprt/gtk/class/global_shortcuts.zig`):
//! raw D-Bus over the *GApplication's own* connection (so the portal sees the
//! app's identity and KDE shows its permission prompt), driven on the GTK main
//! thread. The XDG Request/Response pattern is racy — the Response signal must
//! be subscribed on a manually-constructed request path *before* the call — so
//! we replicate that dance rather than rely on a portal library (ashpd creates
//! a separate connection, and KDE then never prompts).
//!
//! Flow: CreateSession → (Response) subscribe Activated + BindShortcuts (this is
//! what triggers the KDE permission dialog) → (Activated) toggle the drop-down.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use gtk4::gio;
use gtk4::glib;

use crate::app::{QuickTermAction, SharedState, UiEvent};

/// The shortcut id is *derived from the hotkey* rather than fixed. KDE persists
/// global-shortcut state keyed by (app-id, shortcut-id) in kglobalshortcutsrc,
/// and only honours `preferred_trigger` (and only shows the bind dialog) on the
/// *first* registration of a given id — a later BindShortcuts with a changed
/// trigger is silently ignored. Folding the hotkey into the id means every
/// distinct hotkey is a brand-new shortcut to KDE, so changing the hotkey in
/// settings actually re-binds (and re-prompts) instead of reusing a stale key.
fn shortcut_id(hotkey: &str) -> String {
    let slug: String = hotkey
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("quick-terminal-{}", slug.trim_matches('-'))
}
const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const GS_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// Register the quick-terminal global shortcut on `conn`. Idempotent: only the
/// first successful call per process does anything.
pub fn register(conn: &gio::DBusConnection, shared: Arc<SharedState>) {
    static REGISTERED: AtomicBool = AtomicBool::new(false);

    let cfg = crate::settings::load().quick_terminal;
    if !cfg.enabled || cfg.hotkey.trim().is_empty() {
        return;
    }
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    create_session(conn.clone(), shared, sanitize(&cfg.hotkey));
}

fn sanitize(s: &str) -> String {
    s.trim().replace('\'', "")
}

fn gen_token(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("{prefix}{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// `/org/freedesktop/portal/desktop/request/<sanitized-unique-name>/<token>`.
fn request_path(conn: &gio::DBusConnection, token: &str) -> Option<String> {
    let unique = conn.unique_name()?; // e.g. ":1.192"
    let sanitized = unique.trim_start_matches(':').replace('.', "_");
    Some(format!("{PORTAL_PATH}/request/{sanitized}/{token}"))
}

/// Subscribe to the Response signal on `path`, invoking `on_response(results)`
/// once with the results dict when the request succeeds (response code 0).
fn on_request_response<F>(conn: &gio::DBusConnection, path: &str, on_response: F)
where
    F: Fn(&gio::DBusConnection, glib::Variant) + 'static,
{
    // Hold the subscription guard in a cell so the callback can drop it to
    // unsubscribe after firing exactly once (dropping the guard is what the
    // old signal_unsubscribe(id) call did).
    let sub = std::rc::Rc::new(std::cell::Cell::new(None));
    let sub_cb = sub.clone();
    let subscription = conn.subscribe_to_signal(
        None,
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(path),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            // Fire once: drop the subscription guard to unsubscribe.
            drop(sub_cb.take());
            // parameters: (u response, a{sv} results)
            let code = signal.parameters.child_value(0).get::<u32>().unwrap_or(2);
            if code != 0 {
                tracing::warn!(code, "quick terminal: portal request not granted");
                return;
            }
            on_response(signal.connection, signal.parameters.child_value(1));
        },
    );
    sub.set(Some(subscription));
}

fn create_session(conn: gio::DBusConnection, shared: Arc<SharedState>, hotkey: String) {
    let request_token = gen_token("jmux_req_");
    let session_token = gen_token("jmux_sess_");
    let Some(req_path) = request_path(&conn, &request_token) else {
        tracing::warn!("quick terminal: no D-Bus unique name; cannot register global shortcut");
        return;
    };

    // Subscribe to the response *before* calling (the portal may reply first).
    on_request_response(&conn, &req_path, move |conn, results| {
        let dict = glib::VariantDict::new(Some(&results));
        let Some(handle) = dict.lookup::<String>("session_handle").ok().flatten() else {
            tracing::warn!("quick terminal: portal CreateSession returned no session_handle");
            return;
        };
        bind_shortcuts(conn, &handle, shared.clone(), &hotkey);
    });

    let payload = glib::Variant::parse(
        None,
        &format!(
            "({{'handle_token': <'{request_token}'>, 'session_handle_token': <'{session_token}'>}},)"
        ),
    );
    let payload = match payload {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("quick terminal: CreateSession payload error: {e}");
            return;
        }
    };
    conn.call(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        GS_IFACE,
        "CreateSession",
        Some(&payload),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |res| {
            if let Err(e) = res {
                tracing::warn!("quick terminal: CreateSession failed: {e}");
            }
        },
    );
}

fn bind_shortcuts(
    conn: &gio::DBusConnection,
    session_handle: &str,
    shared: Arc<SharedState>,
    hotkey: &str,
) {
    // Toggle whenever our shortcut activates (delivered on the GTK thread).
    // Subscribe broadly to Activated on the interface (no arg0 path filter,
    // which can be finicky) and check the shortcut id ourselves.
    let shortcut_id = shortcut_id(hotkey);
    {
        let shared = shared.clone();
        let want_id = shortcut_id.clone();
        let subscription = conn.subscribe_to_signal(
            None,
            Some(GS_IFACE),
            Some("Activated"),
            Some(PORTAL_PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                // parameters: (o session_handle, s shortcut_id, t timestamp, a{sv})
                let id = signal.parameters.child_value(1).get::<String>();
                tracing::info!(?id, "quick terminal: portal Activated");
                if id.as_deref() == Some(want_id.as_str()) {
                    shared.send_ui_event(UiEvent::QuickTerminal(QuickTermAction::Toggle));
                }
            },
        );
        // Process-lifetime subscription: the hotkey must keep toggling for the
        // life of the app, so leak the guard instead of unsubscribing on drop
        // (mirrors the prior signal_subscribe whose id was intentionally dropped).
        std::mem::forget(subscription);
    }

    let request_token = gen_token("jmux_req_");
    if let Some(req_path) = request_path(conn, &request_token) {
        let session = session_handle.to_string();
        on_request_response(conn, &req_path, move |conn, _results| {
            tracing::info!("quick terminal global shortcut bound");
            list_shortcuts(conn, &session);
        });
    }

    // (o session, a(sa{sv}) shortcuts, s parent_window, a{sv} options)
    let payload = glib::Variant::parse(
        None,
        &format!(
            "(objectpath '{session_handle}', \
             [('{shortcut_id}', {{'description': <'Toggle the jmux quick terminal'>, \
             'preferred_trigger': <'{hotkey}'>}})], \
             '', {{'handle_token': <'{request_token}'>}})"
        ),
    );
    let payload = match payload {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("quick terminal: BindShortcuts payload error: {e}");
            return;
        }
    };
    conn.call(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        GS_IFACE,
        "BindShortcuts",
        Some(&payload),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |res| {
            if let Err(e) = res {
                tracing::warn!("quick terminal: BindShortcuts failed: {e}");
            }
        },
    );
}

/// Diagnostic: ask the portal what shortcuts it has for our session and log the
/// trigger KDE actually assigned (empty trigger ⇒ the user must assign a key).
fn list_shortcuts(conn: &gio::DBusConnection, session_handle: &str) {
    let request_token = gen_token("jmux_req_");
    let Some(req_path) = request_path(conn, &request_token) else {
        return;
    };
    on_request_response(conn, &req_path, |_conn, results| {
        let dict = glib::VariantDict::new(Some(&results));
        let Some(shortcuts) = dict.lookup_value("shortcuts", None) else {
            tracing::info!("quick terminal: ListShortcuts returned no shortcuts");
            return;
        };
        for i in 0..shortcuts.n_children() {
            let entry = shortcuts.child_value(i); // (s a{sv})
            let id = entry.child_value(0).get::<String>().unwrap_or_default();
            let meta = glib::VariantDict::new(Some(&entry.child_value(1)));
            let trigger = meta
                .lookup_value("trigger_description", None)
                .and_then(|v| v.str().map(|s| s.to_string()))
                .unwrap_or_default();
            tracing::info!(id, trigger = %trigger, "quick terminal: KDE-registered shortcut");
        }
    });

    let payload = match glib::Variant::parse(
        None,
        &format!("(objectpath '{session_handle}', {{'handle_token': <'{request_token}'>}})"),
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("quick terminal: ListShortcuts payload error: {e}");
            return;
        }
    };
    conn.call(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        GS_IFACE,
        "ListShortcuts",
        Some(&payload),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        |res| {
            if let Err(e) = res {
                tracing::warn!("quick terminal: ListShortcuts failed: {e}");
            }
        },
    );
}
