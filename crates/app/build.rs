//! Build script — wires Tauri's codegen (context, assets, capabilities)
//! into the normal cargo build.

fn main() {
    tauri_build::build();
}
