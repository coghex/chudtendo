//! FFI declarations for the Qt C++ helper (qt_helper.cpp).
//! Only available when the "qt" feature is enabled.

#[cfg(feature = "qt")]
unsafe extern "C" {
    pub fn qt_init(argc: *mut i32, argv: *mut *mut i8) -> i32;
    pub fn qt_process_events();
    pub fn qt_poll_action() -> i32;
    pub fn qt_create_menubar();
    pub fn qt_destroy_menubar();

    pub fn qt_open_preferences(
        ff_speed: f32,
        rewind_secs: i32,
        boot_rom: *const i8,
        window_scale: i32,
        window_mode: i32,
        vsync: i32,
        frame_limit: i32,
        key_bindings: *const i8,
    );

    pub fn qt_prefs_ff_speed() -> f32;
    pub fn qt_prefs_rewind_secs() -> i32;
    pub fn qt_prefs_boot_rom_is_dmg() -> i32;
    pub fn qt_prefs_scale() -> i32;
    pub fn qt_prefs_window_mode() -> i32;
    pub fn qt_prefs_vsync() -> i32;
    pub fn qt_prefs_frame_limit() -> i32;
    pub fn qt_prefs_key_bindings() -> *mut i8;
    pub fn qt_free_string(s: *mut i8);

    pub fn qt_shutdown();
}
