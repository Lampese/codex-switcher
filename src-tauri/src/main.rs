// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod linux_gtk;

fn main() {
    #[cfg(target_os = "linux")]
    {
        for variable in ["GTK_MODULES", "GTK3_MODULES"] {
            if let Some(filtered) = std::env::var_os(variable)
                .as_deref()
                .and_then(linux_gtk::without_appmenu_module)
            {
                unsafe { std::env::set_var(variable, filtered) };
            }
        }

        // CPU tile painting reduces Skia's GPU allocations while keeping GPU
        // compositing and asynchronous scrolling. Preserve explicit overrides.
        if std::env::var_os("WEBKIT_SKIA_ENABLE_CPU_RENDERING").is_none() {
            unsafe { std::env::set_var("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1") };
        }
    }

    codex_switcher_lib::run()
}
