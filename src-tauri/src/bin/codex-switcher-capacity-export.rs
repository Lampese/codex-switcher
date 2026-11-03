use std::path::PathBuf;

fn argument(name: &str) -> Option<PathBuf> {
    let mut values = std::env::args_os().skip(1);
    while let Some(value) = values.next() {
        if value == name {
            return values.next().map(PathBuf::from);
        }
    }
    None
}

#[tokio::main]
async fn main() {
    let output = argument("--output").unwrap_or_else(|| {
        eprintln!("--output is required");
        std::process::exit(2);
    });
    let admission = argument("--admission");
    if codex_switcher_lib::capacity_export::export_capacity(&output, admission.as_deref())
        .await
        .is_err()
    {
        // Keep every CLI error path identity- and credential-free. Detailed
        // backend errors stay inside the Switcher trust boundary.
        eprintln!("capacity export failed");
        std::process::exit(2);
    }
}
