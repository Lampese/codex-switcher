use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

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
}
