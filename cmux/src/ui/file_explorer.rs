//! File explorer panel — collapsible tree view of the workspace working directory.
//!
//! Uses `gtk4::TreeView` + `gtk4::TreeStore` for lazy (on-demand) directory expansion.
//! SSH workspaces show a placeholder instead of the tree.
//!
//! A preview pane appears below the tree when a file is single-clicked.  It shows:
//! - Text content (first 4 KB) for UTF-8 files
//! - A scaled thumbnail for recognized image formats
//! - A "binary file" notice + "Open with…" button for everything else

use std::path::Path;

use glib::translate::ToGlibPtr;
use gtk4::prelude::*;

// Column indices in the TreeStore
const COL_ICON: u32 = 0;
const COL_NAME: u32 = 1;
const COL_PATH: u32 = 2;
const COL_IS_DIR: u32 = 3;
const COL_HAS_DUMMY: u32 = 4; // whether the dummy child sentinel is present

/// Maximum depth to expand lazily; dirs at this depth show no expand arrow.
const MAX_DEPTH: u32 = 3;

/// Sentinel value stored as the path of a dummy child row.
/// Must not start with \x00 — GTK TreeStore rejects strings with interior NUL bytes.
const DUMMY_PATH: &str = "\x01__cmux_dummy__";

/// Max bytes loaded into the text preview.
const PREVIEW_TEXT_LIMIT: usize = 4096;

/// Image MIME type / extension list used for thumbnail preview.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "tiff", "tif",
];

pub struct FileExplorer {
    /// The outer container (either the tree view + preview, or the SSH placeholder).
    root: gtk4::Widget,
    /// The store — `None` for SSH workspaces.
    store: Option<gtk4::TreeStore>,
    /// The tree view — `None` for SSH workspaces.
    tree_view: Option<gtk4::TreeView>,
}

impl FileExplorer {
    /// Build the widget. The tree is initially empty; call `set_root` to populate it.
    pub fn new() -> Self {
        // Column types: icon-name (String), display-name (String), full path (String),
        // is_dir (bool), has_dummy (bool)
        let store = gtk4::TreeStore::new(&[
            glib::Type::STRING, // icon name
            glib::Type::STRING, // display name
            glib::Type::STRING, // full path
            glib::Type::BOOL,   // is directory
            glib::Type::BOOL,   // has dummy child
        ]);

        let tree_view = gtk4::TreeView::with_model(&store);
        tree_view.set_headers_visible(false);
        tree_view.set_enable_tree_lines(false);
        tree_view.add_css_class("file-tree");

        // Icon column
        let icon_renderer = gtk4::CellRendererPixbuf::new();
        let icon_col = gtk4::TreeViewColumn::new();
        icon_col.pack_start(&icon_renderer, false);
        icon_col.add_attribute(&icon_renderer, "icon-name", COL_ICON as i32);
        tree_view.append_column(&icon_col);

        // Name column
        let text_renderer = gtk4::CellRendererText::new();
        let name_col = gtk4::TreeViewColumn::new();
        name_col.pack_start(&text_renderer, true);
        name_col.add_attribute(&text_renderer, "text", COL_NAME as i32);
        name_col.set_expand(true);
        tree_view.append_column(&name_col);

        // Lazy expand: when a row with a dummy child is expanded, replace it with real children.
        {
            let store_weak = store.downgrade();
            tree_view.connect_row_expanded(move |tv, iter, path| {
                let Some(store) = store_weak.upgrade() else { return };
                let has_dummy = tree_model_get_bool(&store, iter, COL_HAS_DUMMY);
                if !has_dummy {
                    return;
                }
                let full_path = tree_model_get_string(&store, iter, COL_PATH);

                // Remove dummy child
                if let Some(child) = store.iter_children(Some(iter)) {
                    store.remove(&child);
                }
                // Mark no-longer-has-dummy
                store.set(iter, &[(COL_HAS_DUMMY, &false)]);

                // Current depth = number of path components in the TreePath
                let depth = path.depth() as u32; // depth of this row (1-based)
                populate_dir(&store, Some(iter), &full_path, depth);

                // Scroll so the expanded node is visible
                tv.scroll_to_cell(Some(path), None::<&gtk4::TreeViewColumn>, false, 0.0, 0.0);
            });
        }

        // Double-click on a file opens it via xdg-open.
        {
            let store_weak = store.downgrade();
            let gesture = gtk4::GestureClick::new();
            gesture.set_button(1);
            {
                let tree_view_weak = tree_view.downgrade();
                gesture.connect_pressed(move |gesture, n_press, x, y| {
                    if n_press != 2 {
                        return;
                    }
                    let Some(tree_view) = tree_view_weak.upgrade() else { return };
                    let Some(store) = store_weak.upgrade() else { return };
                    if let Some((Some(path), _, _, _)) = tree_view.path_at_pos(x as i32, y as i32)
                    {
                        if let Some(iter) = store.iter(&path) {
                            let is_dir = tree_model_get_bool(&store, &iter, COL_IS_DIR);
                            if !is_dir {
                                let full_path = tree_model_get_string(&store, &iter, COL_PATH);
                                if !full_path.is_empty() && full_path != DUMMY_PATH {
                                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                                    let _ = std::process::Command::new("xdg-open")
                                        .arg(&full_path)
                                        .spawn();
                                }
                            }
                        }
                    }
                });
            }
            tree_view.add_controller(gesture);
        }

        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&tree_view));

        // ── Preview pane ──────────────────────────────────────────────────────
        // Shown below the tree when the user single-clicks a file.

        let preview_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        preview_box.set_visible(false);
        preview_box.add_css_class("file-preview-pane");

        let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
        preview_box.append(&sep);

        // Filename label
        let preview_name = gtk4::Label::new(None);
        preview_name.add_css_class("caption");
        preview_name.add_css_class("dim-label");
        preview_name.set_xalign(0.0);
        preview_name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        preview_name.set_margin_start(6);
        preview_name.set_margin_end(6);
        preview_name.set_margin_top(4);
        preview_name.set_margin_bottom(2);
        preview_box.append(&preview_name);

        // Content area: a stack that can show text, image, or binary notice
        let stack = gtk4::Stack::new();
        stack.set_transition_type(gtk4::StackTransitionType::None);
        stack.set_vexpand(false);

        // Text page
        let text_scroll = gtk4::ScrolledWindow::new();
        text_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
        text_scroll.set_size_request(-1, 120);
        let text_view = gtk4::TextView::new();
        text_view.set_editable(false);
        text_view.set_monospace(true);
        text_view.set_wrap_mode(gtk4::WrapMode::WordChar);
        text_view.set_margin_start(6);
        text_view.set_margin_end(6);
        text_view.set_margin_bottom(4);
        text_view.add_css_class("caption");
        text_scroll.set_child(Some(&text_view));
        stack.add_named(&text_scroll, Some("text"));

        // Image page
        let image_scroll = gtk4::ScrolledWindow::new();
        image_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
        image_scroll.set_size_request(-1, 120);
        let picture = gtk4::Picture::new();
        picture.set_can_shrink(true);
        picture.set_margin_start(6);
        picture.set_margin_end(6);
        picture.set_margin_bottom(4);
        image_scroll.set_child(Some(&picture));
        stack.add_named(&image_scroll, Some("image"));

        // Binary page
        let binary_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        binary_box.set_margin_start(6);
        binary_box.set_margin_end(6);
        binary_box.set_margin_top(4);
        binary_box.set_margin_bottom(4);
        let binary_label = gtk4::Label::new(Some("Binary file"));
        binary_label.add_css_class("dim-label");
        binary_label.add_css_class("caption");
        binary_label.set_xalign(0.0);
        binary_box.append(&binary_label);
        let open_btn = gtk4::Button::with_label("Open with…");
        open_btn.add_css_class("flat");
        open_btn.add_css_class("caption");
        open_btn.set_halign(gtk4::Align::Start);
        binary_box.append(&open_btn);
        stack.add_named(&binary_box, Some("binary"));

        preview_box.append(&stack);

        // Wire single-click on tree to update the preview pane.
        {
            let store_weak = store.downgrade();
            let preview_box_w = preview_box.downgrade();
            let preview_name_w = preview_name.downgrade();
            let text_view_w = text_view.downgrade();
            let picture_w = picture.downgrade();
            let stack_w = stack.downgrade();
            let open_btn_w = open_btn.downgrade();

            tree_view.connect_cursor_changed(move |tv| {
                let Some(store) = store_weak.upgrade() else { return };
                let Some(preview_box) = preview_box_w.upgrade() else { return };
                let Some(preview_name) = preview_name_w.upgrade() else { return };
                let Some(text_view) = text_view_w.upgrade() else { return };
                let Some(picture) = picture_w.upgrade() else { return };
                let Some(stack) = stack_w.upgrade() else { return };
                let Some(open_btn) = open_btn_w.upgrade() else { return };

                let (cursor_path, _) = gtk4::prelude::TreeViewExt::cursor(tv);
                let Some(path) = cursor_path else {
                    preview_box.set_visible(false);
                    return;
                };
                let Some(iter) = store.iter(&path) else {
                    preview_box.set_visible(false);
                    return;
                };

                let is_dir = tree_model_get_bool(&store, &iter, COL_IS_DIR);
                if is_dir {
                    preview_box.set_visible(false);
                    return;
                }

                let full_path = tree_model_get_string(&store, &iter, COL_PATH);
                if full_path.is_empty() || full_path == DUMMY_PATH {
                    preview_box.set_visible(false);
                    return;
                }

                // Show filename
                let name = Path::new(&full_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                preview_name.set_text(&name);

                // Determine content type by extension
                let ext = Path::new(&full_path)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();

                if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                    // Image preview via gtk4::Picture
                    let file = gio::File::for_path(&full_path);
                    picture.set_file(Some(&file));
                    stack.set_visible_child_name("image");
                } else {
                    // Attempt text preview: read up to PREVIEW_TEXT_LIMIT bytes
                    match read_text_preview(&full_path) {
                        Some(text) => {
                            text_view.buffer().set_text(&text);
                            stack.set_visible_child_name("text");
                        }
                        None => {
                            // Binary: wire the "Open with…" button to xdg-open
                            let path_for_open = full_path.clone();
                            open_btn.connect_clicked(move |_| {
                                let _ = std::process::Command::new("xdg-open")
                                    .arg(&path_for_open)
                                    .spawn();
                            });
                            stack.set_visible_child_name("binary");
                        }
                    }
                }

                preview_box.set_visible(true);
            });
        }

        // ── Outer container: tree above, preview below ────────────────────────
        let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        outer.set_vexpand(true);
        outer.append(&scrolled);
        outer.append(&preview_box);

        Self {
            root: outer.upcast(),
            store: Some(store),
            tree_view: Some(tree_view),
        }
    }

    /// Build an SSH-workspace placeholder (no tree).
    ///
    /// When `current_directory` is non-empty, it is displayed as a read-only
    /// path label (tracked remotely via shell integration PWD/OSC reporting).
    /// This gives users the remote CWD at a glance without requiring a live
    /// file-listing RPC call to the remote daemon.
    pub fn new_ssh_placeholder(current_directory: &str) -> Self {
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        vbox.set_margin_top(6);
        vbox.set_margin_bottom(6);
        vbox.set_margin_start(4);
        vbox.set_margin_end(4);

        let info_label = gtk4::Label::new(Some("SSH remote — file browsing unavailable"));
        info_label.add_css_class("dim-label");
        info_label.add_css_class("caption");
        info_label.set_justify(gtk4::Justification::Center);
        info_label.set_wrap(true);
        vbox.append(&info_label);

        // Show the remote CWD when available (updated by shell integration).
        if !current_directory.is_empty() {
            let cwd_label = gtk4::Label::new(Some(current_directory));
            cwd_label.add_css_class("dim-label");
            cwd_label.add_css_class("caption");
            cwd_label.set_selectable(true); // allow copying the path
            cwd_label.set_wrap(true);
            cwd_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
            cwd_label.set_xalign(0.0);
            vbox.append(&cwd_label);
        }

        Self {
            root: vbox.upcast(),
            store: None,
            tree_view: None,
        }
    }

    /// The top-level widget to embed in a parent container.
    pub fn widget(&self) -> &gtk4::Widget {
        &self.root
    }

    /// Repopulate the tree from `path`. Clears any existing contents.
    pub fn set_root(&self, path: &str) {
        let (Some(store), Some(tree_view)) = (self.store.as_ref(), self.tree_view.as_ref()) else {
            return;
        };
        store.clear();
        if path.is_empty() {
            return;
        }
        // Collapse all first so we don't get stale expander state
        tree_view.collapse_all();
        populate_dir(store, None, path, 0);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read up to `PREVIEW_TEXT_LIMIT` bytes from a file and return the content
/// as a UTF-8 string.  Returns `None` if the file cannot be read or if the
/// first `PREVIEW_TEXT_LIMIT` bytes are not valid UTF-8 (i.e. binary).
fn read_text_preview(path: &str) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; PREVIEW_TEXT_LIMIT];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    String::from_utf8(buf).ok()
}

/// Read `dir_path` and append its children as rows under `parent_iter`.
/// `current_depth` is the depth of `parent_iter` (0 = root level).
fn populate_dir(
    store: &gtk4::TreeStore,
    parent_iter: Option<&gtk4::TreeIter>,
    dir_path: &str,
    current_depth: u32,
) {
    let path = Path::new(dir_path);
    let Ok(read_dir) = std::fs::read_dir(path) else {
        return;
    };

    let mut dirs: Vec<(String, String)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip hidden files/dirs starting with "."
        if name.starts_with('.') {
            continue;
        }
        let full = entry.path().to_string_lossy().into_owned();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => dirs.push((name, full)),
            Ok(_) => files.push((name, full)),
            Err(_) => {}
        }
    }

    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let next_depth = current_depth + 1;

    for (name, full) in dirs {
        let iter = store.append(parent_iter);
        store.set(
            &iter,
            &[
                (COL_ICON, &"folder-symbolic"),
                (COL_NAME, &name.as_str()),
                (COL_PATH, &full.as_str()),
                (COL_IS_DIR, &true),
                (COL_HAS_DUMMY, &false),
            ],
        );
        if next_depth < MAX_DEPTH {
            // Add a dummy child so the expander arrow appears; real children
            // are populated lazily when the user expands this row.
            let dummy = store.append(Some(&iter));
            store.set(
                &dummy,
                &[
                    (COL_ICON, &""),
                    (COL_NAME, &"…"),
                    (COL_PATH, &DUMMY_PATH),
                    (COL_IS_DIR, &false),
                    (COL_HAS_DUMMY, &false),
                ],
            );
            store.set(&iter, &[(COL_HAS_DUMMY, &true)]);
        }
        // At MAX_DEPTH we add a static "…" leaf to signal there are children.
        if next_depth >= MAX_DEPTH {
            let ellipsis = store.append(Some(&iter));
            store.set(
                &ellipsis,
                &[
                    (COL_ICON, &""),
                    (COL_NAME, &"…"),
                    (COL_PATH, &DUMMY_PATH),
                    (COL_IS_DIR, &false),
                    (COL_HAS_DUMMY, &false),
                ],
            );
        }
    }

    for (name, full) in files {
        let iter = store.append(parent_iter);
        store.set(
            &iter,
            &[
                (COL_ICON, &"text-x-generic-symbolic"),
                (COL_NAME, &name.as_str()),
                (COL_PATH, &full.as_str()),
                (COL_IS_DIR, &false),
                (COL_HAS_DUMMY, &false),
            ],
        );
    }
}

// ---------------------------------------------------------------------------
// GTK TreeModel value accessors
//
// The `value()` method lives on `TreeModelExt` which requires `IsA<TreeModel>`.
// Due to trait-name collisions in `gtk4::prelude::*` (multiple traits expose a
// `value` method), calling `.value()` directly on `TreeStore` is ambiguous.
// Using unsafe FFI to read GValue directly sidesteps the ambiguity cleanly.
// ---------------------------------------------------------------------------

/// Read a `bool` column from a `TreeStore` row via raw GTK FFI.
///
/// We use raw FFI because `TreeModelExt::value` is ambiguous in the presence
/// of `gtk4::prelude::*` (multiple traits expose a `value` method with the
/// same name, preventing method resolution).
fn tree_model_get_bool(store: &gtk4::TreeStore, iter: &gtk4::TreeIter, col: u32) -> bool {
    unsafe {
        let raw_model = store.as_ptr() as *mut gtk4::ffi::GtkTreeModel;
        let raw_iter = iter.to_glib_none().0 as *mut gtk4::ffi::GtkTreeIter;
        let mut value = std::mem::MaybeUninit::<glib::gobject_ffi::GValue>::zeroed();
        gtk4::ffi::gtk_tree_model_get_value(raw_model, raw_iter, col as std::ffi::c_int, value.as_mut_ptr());
        let v = value.assume_init();
        let result = glib::gobject_ffi::g_value_get_boolean(&v) != 0;
        glib::gobject_ffi::g_value_unset(value.as_mut_ptr());
        result
    }
}

/// Read a `String` column from a `TreeStore` row via raw GTK FFI.
fn tree_model_get_string(store: &gtk4::TreeStore, iter: &gtk4::TreeIter, col: u32) -> String {
    unsafe {
        let raw_model = store.as_ptr() as *mut gtk4::ffi::GtkTreeModel;
        let raw_iter = iter.to_glib_none().0 as *mut gtk4::ffi::GtkTreeIter;
        let mut value = std::mem::MaybeUninit::<glib::gobject_ffi::GValue>::zeroed();
        gtk4::ffi::gtk_tree_model_get_value(raw_model, raw_iter, col as std::ffi::c_int, value.as_mut_ptr());
        let v = value.assume_init();
        let ptr = glib::gobject_ffi::g_value_get_string(&v);
        let result = if ptr.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(ptr)
                .to_string_lossy()
                .into_owned()
        };
        glib::gobject_ffi::g_value_unset(value.as_mut_ptr());
        result
    }
}
