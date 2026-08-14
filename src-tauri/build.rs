fn main() {
    let app_manifest = tauri_build::AppManifest::new().commands(&[
        "notify",
        "set_unread",
        "dlog",
        "get_settings",
        "set_settings",
        "open_settings",
        "list_accounts",
        "add_account",
        "remove_account",
        "rename_account",
        "open_account",
        "get_lock_status",
        "set_app_lock_password",
        "change_app_lock_password",
        "disable_app_lock",
        "set_app_lock_options",
        "set_biometric_enabled",
        "lock_app",
        "unlock_password",
        "unlock_biometric",
        "reset_app_lock",
    ]);
    let attributes = tauri_build::Attributes::new().app_manifest(app_manifest);
    tauri_build::try_build(attributes).expect("failed to run Tauri build script");
}
