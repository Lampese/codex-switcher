use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

/// Which env vars the AppImage sanitizer removes/sets (pure, unit-testable).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AppimageEnvPlan {
    pub remove: Vec<&'static str>,
    pub set: Vec<(&'static str, &'static str)>,
}

/// Compute the AppImage env fix-up plan (#195) from the current variable
/// values, without touching the environment.
///
/// The linuxdeploy GTK hook baked into our AppImage forces `GDK_BACKEND=x11`
/// and points `GIO_MODULE_DIR` at host modules. On Wayland hosts this makes
/// WebKitGTK's WebProcess abort with `Could not create default EGL display:
/// EGL_BAD_PARAMETER`, and the bundled (older) GLib fails to resolve symbols
/// required by host GVFS modules (`g_task_set_static_name`).
pub fn appimage_env_plan(get: impl Fn(&str) -> Option<String>) -> AppimageEnvPlan {
    let mut plan = AppimageEnvPlan::default();
    // Only inside an AppImage: `.deb`/user builds keep their system config.
    if get("APPIMAGE").is_none() {
        return plan;
    }

    // Host GIO/GVFS modules are linked against the host GLib, which is newer
    // than the bundled one — loading them crashes symbol resolution.
    plan.remove
        .extend(["GIO_MODULE_DIR", "GIO_EXTRA_MODULES", "GVFS_MODULE_DIR"]);

    // Under Wayland, let GTK pick the backend itself instead of honoring the
    // hook's forced `x11` value that trips the EGL fallback path.
    if get("WAYLAND_DISPLAY").is_some() && get("GDK_BACKEND").as_deref() == Some("x11") {
        plan.remove.push("GDK_BACKEND");
    }

    // WebKitGTK DMA-BUF renderer aborts with EGL_BAD_PARAMETER on several
    // Wayland/XWayland driver combinations; disabling it is the standard
    // workaround until the bundled WebKitGTK is fixed upstream.
    if get("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        plan.set.push(("WEBKIT_DISABLE_DMABUF_RENDERER", "1"));
    }
    plan
}

/// Apply [`appimage_env_plan`] to the real process environment.
pub fn sanitize_appimage_env() {
    let plan = appimage_env_plan(|key| std::env::var(key).ok());
    for variable in plan.remove {
        unsafe { std::env::remove_var(variable) };
    }
    for (key, value) in plan.set {
        unsafe { std::env::set_var(key, value) };
    }
}

pub fn without_appmenu_module(value: &OsStr) -> Option<OsString> {
    let modules: Vec<_> = value.as_bytes().split(|byte| *byte == b':').collect();
    let filtered: Vec<_> = modules
        .iter()
        .copied()
        .filter(|module| {
            let name = module
                .trim_ascii()
                .rsplit(|byte| *byte == b'/')
                .next()
                .unwrap_or_default();
            !matches!(
                name,
                b"appmenu-gtk-module"
                    | b"appmenu-gtk-module.so"
                    | b"libappmenu-gtk-module"
                    | b"libappmenu-gtk-module.so"
            )
        })
        .collect();

    (filtered.len() != modules.len()).then(|| OsString::from_vec(filtered.join(&b':')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_accessibility_modules_from_reported_environment() {
        assert_eq!(
            without_appmenu_module(OsStr::new("gail:atk-bridge:appmenu-gtk-module")),
            Some(OsString::from("gail:atk-bridge"))
        );
    }

    #[test]
    fn removes_module_at_any_position() {
        for value in [
            "appmenu-gtk-module:gail:atk-bridge",
            "gail:appmenu-gtk-module:atk-bridge",
            "gail:atk-bridge:appmenu-gtk-module",
        ] {
            assert_eq!(
                without_appmenu_module(OsStr::new(value)),
                Some(OsString::from("gail:atk-bridge"))
            );
        }
    }

    #[test]
    fn recognizes_library_names_and_paths() {
        for module in [
            "appmenu-gtk-module",
            "appmenu-gtk-module.so",
            "libappmenu-gtk-module",
            "libappmenu-gtk-module.so",
            "/usr/lib/x86_64-linux-gnu/gtk-3.0/modules/libappmenu-gtk-module.so",
            "./modules/libappmenu-gtk-module.so",
            " \tappmenu-gtk-module \t",
        ] {
            let value = format!("gail:{module}:atk-bridge");
            assert_eq!(
                without_appmenu_module(OsStr::new(&value)),
                Some(OsString::from("gail:atk-bridge")),
                "{module}"
            );
        }
    }

    #[test]
    fn removes_all_occurrences_including_module_only_lists() {
        assert_eq!(
            without_appmenu_module(OsStr::new("appmenu-gtk-module:libappmenu-gtk-module.so")),
            Some(OsString::new())
        );
        assert_eq!(
            without_appmenu_module(OsStr::new("appmenu-gtk-module:gail:appmenu-gtk-module")),
            Some(OsString::from("gail"))
        );
    }

    #[test]
    fn leaves_unrelated_and_empty_values_unchanged() {
        for value in [
            "",
            "gail:atk-bridge",
            ":gail::atk-bridge:",
            "my-appmenu-gtk-module:appmenu-gtk-module-extra",
            "libappmenu-gtk-module.so.backup",
            "/opt/appmenu-gtk-module/other.so",
        ] {
            assert_eq!(without_appmenu_module(OsStr::new(value)), None, "{value}");
        }
    }

    #[test]
    fn preserves_remaining_entries_byte_for_byte() {
        assert_eq!(
            without_appmenu_module(OsStr::new(":gail::appmenu-gtk-module: atk-bridge :")),
            Some(OsString::from(":gail:: atk-bridge :"))
        );
    }

    #[test]
    fn preserves_non_utf8_module_paths() {
        assert_eq!(
            without_appmenu_module(OsStr::from_bytes(
                b"/opt/\xff/other.so:appmenu-gtk-module:atk-bridge"
            )),
            Some(OsString::from_vec(b"/opt/\xff/other.so:atk-bridge".to_vec()))
        );
        assert_eq!(
            without_appmenu_module(OsStr::from_bytes(b"/opt/\xff/other.so")),
            None
        );
    }

    #[test]
    fn filtering_is_idempotent() {
        let filtered = without_appmenu_module(OsStr::new("gail:appmenu-gtk-module")).unwrap();
        assert_eq!(without_appmenu_module(&filtered), None);
    }

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn appimage_env_plan_noop_outside_appimage() {
        let plan = appimage_env_plan(env_of(&[
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("GDK_BACKEND", "x11"),
        ]));
        assert_eq!(plan, AppimageEnvPlan::default());
    }

    #[test]
    fn appimage_env_plan_strips_host_gio_modules_and_dmabuf() {
        let plan = appimage_env_plan(env_of(&[
            ("APPIMAGE", "/opt/Codex.Switcher.AppImage"),
            ("GIO_MODULE_DIR", "/usr/lib/x86_64-linux-gnu/gio/modules"),
        ]));
        assert!(plan.remove.contains(&"GIO_MODULE_DIR"));
        assert!(plan.remove.contains(&"GIO_EXTRA_MODULES"));
        assert!(plan.remove.contains(&"GVFS_MODULE_DIR"));
        assert_eq!(plan.set, vec![("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]);
        // No Wayland session: the forced x11 backend is left alone.
        assert!(!plan.remove.contains(&"GDK_BACKEND"));
    }

    #[test]
    fn appimage_env_plan_drops_forced_x11_only_on_wayland() {
        let wayland = appimage_env_plan(env_of(&[
            ("APPIMAGE", "/opt/app.AppImage"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("GDK_BACKEND", "x11"),
        ]));
        assert!(wayland.remove.contains(&"GDK_BACKEND"));

        // A user-chosen backend is respected.
        let custom = appimage_env_plan(env_of(&[
            ("APPIMAGE", "/opt/app.AppImage"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("GDK_BACKEND", "wayland"),
        ]));
        assert!(!custom.remove.contains(&"GDK_BACKEND"));
    }

    #[test]
    fn appimage_env_plan_respects_existing_dmabuf_override() {
        let plan = appimage_env_plan(env_of(&[
            ("APPIMAGE", "/opt/app.AppImage"),
            ("WEBKIT_DISABLE_DMABUF_RENDERER", "0"),
        ]));
        assert!(plan.set.is_empty());
    }
}
