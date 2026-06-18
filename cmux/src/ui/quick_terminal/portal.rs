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

const SHORTCUT_ID: &str = "toggle-quick-terminal";
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
    let sub_id = std::rc::Rc::new(std::cell::Cell::new(None));
    let sub_id_cb = sub_id.clone();
    let id = conn.signal_subscribe(
        None,
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(path),
        None,
        gio::DBusSignalFlags::NONE,
        move |conn, _sender, _path, _iface, _signal, params| {
            // Fire once.
            if let Some(id) = sub_id_cb.take() {
                conn.signal_unsubscribe(id);
            }
            // params: (u response, a{sv} results)
            let code = params.child_value(0).get::<u32>().unwrap_or(2);
            if code != 0 {
                tracing::warn!(code, "quick terminal: portal request not granted");
                return;
            }
            on_response(conn, params.child_value(1));
        },
    );
    sub_id.set(Some(id));
}

fn create_session(conn: gio::DBusConnection, shared: Arc<SharedState>, hotkey: String) {
    let request_token = gen_token("cmux_req_");
    let session_token = gen_token("cmux_sess_");
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
    {
        let shared = shared.clone();
        conn.signal_subscribe(
            Some(PORTAL_DEST),
            Some(GS_IFACE),
            Some("Activated"),
            Some(PORTAL_PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |_conn, _sender, _path, _iface, _signal, params| {
                // params: (o session_handle, s shortcut_id, t timestamp, a{sv})
                let id = params.child_value(1).get::<String>();
                tracing::info!(?id, "quick terminal: portal Activated");
                if id.as_deref() == Some(SHORTCUT_ID) {
                    shared.send_ui_event(UiEvent::QuickTerminal(QuickTermAction::Toggle));
                }
            },
        );
    }

    let request_token = gen_token("cmux_req_");
    if let Some(req_path) = request_path(conn, &request_token) {
        on_request_response(conn, &req_path, |_conn, _results| {
            tracing::info!("quick terminal global shortcut bound");
        });
    }

    // (o session, a(sa{sv}) shortcuts, s parent_window, a{sv} options)
    let payload = glib::Variant::parse(
        None,
        &format!(
            "(objectpath '{session_handle}', \
             [('{SHORTCUT_ID}', {{'description': <'Toggle the cmux quick terminal'>, \
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
