//! Native macOS preferences window built with AppKit via cocoa/objc.
//!
//! Opens a non-modal NSWindow with an NSTabView containing three tabs:
//! Emulation, Display, and Controls.  Each tab has its own Save and
//! Defaults buttons.

/// Notification sent when settings are saved from the preferences window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsChanged {
    Emulation,
    Display,
    Controls,
}

/// Install the settings-changed channel.  Call once at app startup.
/// Returns a receiver the event loop should poll.
pub fn install_settings_channel() -> std::sync::mpsc::Receiver<SettingsChanged> {
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    #[cfg(any(
        all(target_os = "macos", not(feature = "qt")),
        target_os = "windows",
        feature = "qt",
    ))]
    {
        *platform::SETTINGS_SENDER.lock().unwrap() = Some(tx);
    }
    #[cfg(not(any(
        all(target_os = "macos", not(feature = "qt")),
        target_os = "windows",
        feature = "qt",
    )))]
    {
        let _ = tx;
    }
    rx
}

#[cfg(all(target_os = "macos", not(feature = "qt")))]
mod platform {
    use cocoa::appkit::*;
    use cocoa::base::{id, nil, selector, BOOL, NO, YES};
    use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};

    use crate::settings::{
        Action, ControlsSettings, DisplaySettings, EmulationSettings, WindowMode,
    };

    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // Statics — store UI element references for callbacks
    // -----------------------------------------------------------------------

    struct Refs {
        // Emulation tab
        ff_speed_popup: id,
        rewind_popup: id,
        boot_rom_popup: id,
        // Display tab
        scale_popup: id,
        window_mode_popup: id,
        vsync_checkbox: id,
        frame_limit_field: id,
        // Controls tab — parallel arrays: action enum + shortcut label
        control_labels: Vec<(Action, id)>,
    }

    unsafe impl Send for Refs {}

    pub static SETTINGS_SENDER: Mutex<Option<std::sync::mpsc::SyncSender<super::SettingsChanged>>> =
        Mutex::new(None);

    fn notify(kind: super::SettingsChanged) {
        if let Some(ref tx) = *SETTINGS_SENDER.lock().unwrap() {
            let _ = tx.try_send(kind);
        }
    }

    static REFS: Mutex<Option<Refs>> = Mutex::new(None);

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    const WIN_W: f64 = 520.0;
    const WIN_H: f64 = 420.0;
    const LABEL_W: f64 = 180.0;
    const CTRL_W: f64 = 200.0;
    const ROW_H: f64 = 28.0;
    const PAD: f64 = 20.0;
    const BTN_W: f64 = 100.0;
    const BTN_H: f64 = 32.0;

    const FF_SPEEDS: &[(&str, f32)] = &[
        ("2x", 2.0),
        ("4x", 4.0),
        ("8x", 8.0),
        ("Uncapped", 0.0),
    ];

    const REWIND_SIZES: &[(&str, u32)] = &[
        ("10 seconds", 10),
        ("30 seconds", 30),
        ("60 seconds", 60),
        ("120 seconds", 120),
    ];

    const SCALES: &[(&str, u32)] = &[
        ("1x", 1),
        ("2x", 2),
        ("3x", 3),
        ("4x", 4),
        ("5x", 5),
        ("6x", 6),
        ("7x", 7),
        ("8x", 8),
    ];

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    unsafe fn ns(s: &str) -> id {
        NSString::alloc(nil).init_str(s)
    }

    unsafe fn make_label(text: &str, frame: NSRect) -> id {
        let label: id = msg_send![class!(NSTextField), alloc];
        let label: id = msg_send![label, initWithFrame: frame];
        let () = msg_send![label, setStringValue: ns(text)];
        let () = msg_send![label, setBezeled: NO];
        let () = msg_send![label, setDrawsBackground: NO];
        let () = msg_send![label, setEditable: NO];
        let () = msg_send![label, setSelectable: NO];
        let alignment: u64 = 2; // NSTextAlignmentRight
        let () = msg_send![label, setAlignment: alignment];
        label
    }

    unsafe fn make_popup(items: &[&str], frame: NSRect) -> id {
        let popup: id = msg_send![class!(NSPopUpButton), alloc];
        let popup: id = msg_send![popup, initWithFrame: frame pullsDown: NO];
        for item in items {
            let () = msg_send![popup, addItemWithTitle: ns(item)];
        }
        popup
    }

    unsafe fn make_button(title: &str, frame: NSRect, action: Sel, target: id) -> id {
        let btn: id = msg_send![class!(NSButton), alloc];
        let btn: id = msg_send![btn, initWithFrame: frame];
        let () = msg_send![btn, setTitle: ns(title)];
        let () = msg_send![btn, setBezelStyle: 1i64]; // NSBezelStyleRounded
        let () = msg_send![btn, setAction: action];
        let () = msg_send![btn, setTarget: target];
        btn
    }

    unsafe fn make_checkbox(title: &str, frame: NSRect) -> id {
        let btn: id = msg_send![class!(NSButton), alloc];
        let btn: id = msg_send![btn, initWithFrame: frame];
        let () = msg_send![btn, setTitle: ns(title)];
        let () = msg_send![btn, setButtonType: 3i64]; // NSSwitchButton
        btn
    }

    unsafe fn make_text_field(frame: NSRect) -> id {
        let field: id = msg_send![class!(NSTextField), alloc];
        let field: id = msg_send![field, initWithFrame: frame];
        let () = msg_send![field, setBezeled: YES];
        let () = msg_send![field, setDrawsBackground: YES];
        let () = msg_send![field, setEditable: YES];
        field
    }

    unsafe fn popup_index(popup: id) -> i64 {
        msg_send![popup, indexOfSelectedItem]
    }

    unsafe fn set_popup_index(popup: id, idx: i64) {
        let () = msg_send![popup, selectItemAtIndex: idx];
    }

    unsafe fn string_value(field: id) -> String {
        let val: id = msg_send![field, stringValue];
        let cstr: *const i8 = msg_send![val, UTF8String];
        std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .into_owned()
    }

    // Row y coordinate (rows count from top of content area).
    fn row_y(content_h: f64, row: usize) -> f64 {
        content_h - PAD - (row as f64 + 1.0) * (ROW_H + 6.0)
    }

    // -----------------------------------------------------------------------
    // Tab builders
    // -----------------------------------------------------------------------

    unsafe fn build_emulation_tab(delegate: id) -> id {
        let content_h = WIN_H - 60.0; // tab bar eats ~60px
        let view: id = msg_send![class!(NSView), alloc];
        let view: id = msg_send![view, initWithFrame: NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WIN_W, content_h),
        )];

        let settings = EmulationSettings::load();

        // Row 0: FF Speed
        let y = row_y(content_h, 0);
        let lbl = make_label("Fast Forward Speed:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let popup = make_popup(
            &FF_SPEEDS.iter().map(|&(n, _)| n).collect::<Vec<_>>(),
            NSRect::new(NSPoint::new(PAD + LABEL_W + 10.0, y), NSSize::new(CTRL_W, ROW_H)),
        );
        let idx = FF_SPEEDS.iter().position(|&(_, v)| v == settings.ff_speed).unwrap_or(3) as i64;
        set_popup_index(popup, idx);
        let () = msg_send![view, addSubview: popup];
        let ff_popup = popup;

        // Row 1: Rewind Buffer
        let y = row_y(content_h, 1);
        let lbl = make_label("Rewind Buffer:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let popup = make_popup(
            &REWIND_SIZES.iter().map(|&(n, _)| n).collect::<Vec<_>>(),
            NSRect::new(NSPoint::new(PAD + LABEL_W + 10.0, y), NSSize::new(CTRL_W, ROW_H)),
        );
        let idx = REWIND_SIZES.iter().position(|&(_, v)| v == settings.rewind_buffer_seconds).unwrap_or(1) as i64;
        set_popup_index(popup, idx);
        let () = msg_send![view, addSubview: popup];
        let rewind_popup = popup;

        // Row 2: Boot ROM
        let y = row_y(content_h, 2);
        let lbl = make_label("Boot ROM:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let popup = make_popup(
            &["CGB (Color)", "DMG (Original)"],
            NSRect::new(NSPoint::new(PAD + LABEL_W + 10.0, y), NSSize::new(CTRL_W, ROW_H)),
        );
        let idx = if settings.boot_rom == "dmg" { 1 } else { 0 };
        set_popup_index(popup, idx as i64);
        let () = msg_send![view, addSubview: popup];
        let boot_popup = popup;

        // Save / Defaults buttons
        let btn_y = 12.0;
        let save_btn = make_button("Save", NSRect::new(
            NSPoint::new(WIN_W - PAD - BTN_W, btn_y),
            NSSize::new(BTN_W, BTN_H),
        ), selector("saveEmulation:"), delegate);
        let () = msg_send![view, addSubview: save_btn];

        let defaults_btn = make_button("Defaults", NSRect::new(
            NSPoint::new(WIN_W - PAD - BTN_W * 2.0 - 10.0, btn_y),
            NSSize::new(BTN_W, BTN_H),
        ), selector("defaultsEmulation:"), delegate);
        let () = msg_send![view, addSubview: defaults_btn];

        // Store refs
        {
            let mut refs = REFS.lock().unwrap();
            if let Some(r) = refs.as_mut() {
                r.ff_speed_popup = ff_popup;
                r.rewind_popup = rewind_popup;
                r.boot_rom_popup = boot_popup;
            }
        }

        view
    }

    unsafe fn build_display_tab(delegate: id) -> id {
        let content_h = WIN_H - 60.0;
        let view: id = msg_send![class!(NSView), alloc];
        let view: id = msg_send![view, initWithFrame: NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WIN_W, content_h),
        )];

        let settings = DisplaySettings::load();

        // Row 0: Scale
        let y = row_y(content_h, 0);
        let lbl = make_label("Window Scale:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let popup = make_popup(
            &SCALES.iter().map(|&(n, _)| n).collect::<Vec<_>>(),
            NSRect::new(NSPoint::new(PAD + LABEL_W + 10.0, y), NSSize::new(CTRL_W, ROW_H)),
        );
        let idx = SCALES.iter().position(|&(_, v)| v == settings.window_scale).unwrap_or(3) as i64;
        set_popup_index(popup, idx);
        let () = msg_send![view, addSubview: popup];
        let scale_popup = popup;

        // Row 1: Window Mode
        let y = row_y(content_h, 1);
        let lbl = make_label("Window Mode:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let popup = make_popup(
            &WindowMode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>(),
            NSRect::new(NSPoint::new(PAD + LABEL_W + 10.0, y), NSSize::new(CTRL_W, ROW_H)),
        );
        let idx = WindowMode::ALL.iter().position(|&m| m == settings.window_mode).unwrap_or(0) as i64;
        set_popup_index(popup, idx);
        let () = msg_send![view, addSubview: popup];
        let mode_popup = popup;

        // Row 2: VSync checkbox
        let y = row_y(content_h, 2);
        let lbl = make_label("VSync:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let checkbox = make_checkbox("Enabled", NSRect::new(
            NSPoint::new(PAD + LABEL_W + 10.0, y),
            NSSize::new(CTRL_W, ROW_H),
        ));
        let state: i64 = if settings.vsync { 1 } else { 0 };
        let () = msg_send![checkbox, setState: state];
        let () = msg_send![view, addSubview: checkbox];
        let vsync_cb = checkbox;

        // Row 3: Frame Limit
        let y = row_y(content_h, 3);
        let lbl = make_label("Frame Limit:", NSRect::new(NSPoint::new(PAD, y), NSSize::new(LABEL_W, ROW_H)));
        let () = msg_send![view, addSubview: lbl];
        let field = make_text_field(NSRect::new(
            NSPoint::new(PAD + LABEL_W + 10.0, y),
            NSSize::new(80.0, ROW_H),
        ));
        let val = if settings.frame_limit == 0 {
            "0".to_owned()
        } else {
            settings.frame_limit.to_string()
        };
        let () = msg_send![field, setStringValue: ns(&val)];
        let () = msg_send![view, addSubview: field];
        let fl_field = field;

        // Hint label
        let hint = make_label("(0 = unlimited)", NSRect::new(
            NSPoint::new(PAD + LABEL_W + 100.0, y),
            NSSize::new(120.0, ROW_H),
        ));
        let alignment: u64 = 0; // NSTextAlignmentLeft
        let () = msg_send![hint, setAlignment: alignment];
        let () = msg_send![view, addSubview: hint];

        // Save / Defaults
        let btn_y = 12.0;
        let save_btn = make_button("Save", NSRect::new(
            NSPoint::new(WIN_W - PAD - BTN_W, btn_y),
            NSSize::new(BTN_W, BTN_H),
        ), selector("saveDisplay:"), delegate);
        let () = msg_send![view, addSubview: save_btn];

        let defaults_btn = make_button("Defaults", NSRect::new(
            NSPoint::new(WIN_W - PAD - BTN_W * 2.0 - 10.0, btn_y),
            NSSize::new(BTN_W, BTN_H),
        ), selector("defaultsDisplay:"), delegate);
        let () = msg_send![view, addSubview: defaults_btn];

        {
            let mut refs = REFS.lock().unwrap();
            if let Some(r) = refs.as_mut() {
                r.scale_popup = scale_popup;
                r.window_mode_popup = mode_popup;
                r.vsync_checkbox = vsync_cb;
                r.frame_limit_field = fl_field;
            }
        }

        view
    }

    unsafe fn build_controls_tab(delegate: id) -> id {
        let content_h = WIN_H - 60.0;
        let view: id = msg_send![class!(NSView), alloc];
        let view: id = msg_send![view, initWithFrame: NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WIN_W, content_h),
        )];

        let settings = ControlsSettings::load();

        // Scrollable area with rows
        let scroll_h = content_h - BTN_H - 30.0;
        let scroll: id = msg_send![class!(NSScrollView), alloc];
        let scroll: id = msg_send![scroll, initWithFrame: NSRect::new(
            NSPoint::new(PAD, BTN_H + 20.0),
            NSSize::new(WIN_W - PAD * 2.0, scroll_h),
        )];
        let () = msg_send![scroll, setHasVerticalScroller: YES];
        let () = msg_send![scroll, setBorderType: 1i64]; // NSBezelBorder

        let row_count = Action::ALL.len();
        let doc_h = (row_count as f64) * (ROW_H + 4.0) + PAD;
        let doc_w = WIN_W - PAD * 2.0 - 20.0;

        let doc_view: id = msg_send![class!(NSView), alloc];
        let doc_view: id = msg_send![doc_view, initWithFrame: NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(doc_w, doc_h),
        )];

        let mut control_labels = Vec::new();

        // Header
        let header_y = doc_h - ROW_H - 2.0;
        let action_header = make_label("Action", NSRect::new(
            NSPoint::new(10.0, header_y),
            NSSize::new(200.0, ROW_H),
        ));
        let alignment: u64 = 0; // NSTextAlignmentLeft
        let () = msg_send![action_header, setAlignment: alignment];
        let font: id = msg_send![class!(NSFont), boldSystemFontOfSize: 13.0f64];
        let () = msg_send![action_header, setFont: font];
        let () = msg_send![doc_view, addSubview: action_header];

        let shortcut_header = make_label("Shortcut", NSRect::new(
            NSPoint::new(220.0, header_y),
            NSSize::new(200.0, ROW_H),
        ));
        let () = msg_send![shortcut_header, setAlignment: alignment];
        let () = msg_send![shortcut_header, setFont: font];
        let () = msg_send![doc_view, addSubview: shortcut_header];

        for (i, &action) in Action::ALL.iter().enumerate() {
            let y = doc_h - (i as f64 + 2.0) * (ROW_H + 4.0);

            // Action name label
            let name_lbl = make_label(action.name(), NSRect::new(
                NSPoint::new(10.0, y),
                NSSize::new(200.0, ROW_H),
            ));
            let () = msg_send![name_lbl, setAlignment: alignment];
            let () = msg_send![doc_view, addSubview: name_lbl];

            // Shortcut button — click to rebind
            let key_name = settings.key_for(action);
            let btn = make_button(
                &format!("  {}  ", key_name),
                NSRect::new(NSPoint::new(220.0, y), NSSize::new(180.0, ROW_H)),
                selector("clickShortcut:"),
                delegate,
            );
            let () = msg_send![btn, setBezelStyle: 4i64]; // NSBezelStyleSmallSquare
            // Store the action index as the tag so the callback knows which row.
            let () = msg_send![btn, setTag: i as i64];
            let () = msg_send![doc_view, addSubview: btn];

            control_labels.push((action, btn));
        }

        let () = msg_send![scroll, setDocumentView: doc_view];
        let () = msg_send![view, addSubview: scroll];

        // Save / Defaults
        let btn_y = 12.0;
        let save_btn = make_button("Save", NSRect::new(
            NSPoint::new(WIN_W - PAD - BTN_W, btn_y),
            NSSize::new(BTN_W, BTN_H),
        ), selector("saveControls:"), delegate);
        let () = msg_send![view, addSubview: save_btn];

        let defaults_btn = make_button("Defaults", NSRect::new(
            NSPoint::new(WIN_W - PAD - BTN_W * 2.0 - 10.0, btn_y),
            NSSize::new(BTN_W, BTN_H),
        ), selector("defaultsControls:"), delegate);
        let () = msg_send![view, addSubview: defaults_btn];

        {
            let mut refs = REFS.lock().unwrap();
            if let Some(r) = refs.as_mut() {
                r.control_labels = control_labels;
            }
        }

        view
    }

    // -----------------------------------------------------------------------
    // Delegate callbacks
    // -----------------------------------------------------------------------

    extern "C" fn save_emulation(_this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let refs = REFS.lock().unwrap();
            let Some(r) = refs.as_ref() else { return };
            let ff_idx = popup_index(r.ff_speed_popup) as usize;
            let rw_idx = popup_index(r.rewind_popup) as usize;
            let br_idx = popup_index(r.boot_rom_popup) as usize;
            let settings = EmulationSettings {
                ff_speed: FF_SPEEDS.get(ff_idx).map(|&(_, v)| v).unwrap_or(0.0),
                rewind_buffer_seconds: REWIND_SIZES.get(rw_idx).map(|&(_, v)| v).unwrap_or(30),
                boot_rom: if br_idx == 1 { "dmg" } else { "cgb" }.to_owned(),
            };
            settings.save();
            eprintln!("Emulation settings saved");
            notify(super::SettingsChanged::Emulation);
        }
    }

    extern "C" fn defaults_emulation(_this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let refs = REFS.lock().unwrap();
            let Some(r) = refs.as_ref() else { return };
            let d = EmulationSettings::default();
            let idx = FF_SPEEDS.iter().position(|&(_, v)| v == d.ff_speed).unwrap_or(3) as i64;
            set_popup_index(r.ff_speed_popup, idx);
            let idx = REWIND_SIZES.iter().position(|&(_, v)| v == d.rewind_buffer_seconds).unwrap_or(1) as i64;
            set_popup_index(r.rewind_popup, idx);
            let idx = if d.boot_rom == "dmg" { 1 } else { 0 };
            set_popup_index(r.boot_rom_popup, idx as i64);
        }
    }

    extern "C" fn save_display(_this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let refs = REFS.lock().unwrap();
            let Some(r) = refs.as_ref() else { return };
            let scale_idx = popup_index(r.scale_popup) as usize;
            let mode_idx = popup_index(r.window_mode_popup) as usize;
            let vsync_state: i64 = msg_send![r.vsync_checkbox, state];
            let fl_str = string_value(r.frame_limit_field);
            let settings = DisplaySettings {
                window_scale: SCALES.get(scale_idx).map(|&(_, v)| v).unwrap_or(4),
                window_mode: WindowMode::ALL.get(mode_idx).copied().unwrap_or(WindowMode::Windowed),
                vsync: vsync_state != 0,
                frame_limit: fl_str.parse().unwrap_or(0),
            };
            settings.save();
            eprintln!("Display settings saved");
            notify(super::SettingsChanged::Display);
        }
    }

    extern "C" fn defaults_display(_this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let refs = REFS.lock().unwrap();
            let Some(r) = refs.as_ref() else { return };
            let d = DisplaySettings::default();
            let idx = SCALES.iter().position(|&(_, v)| v == d.window_scale).unwrap_or(3) as i64;
            set_popup_index(r.scale_popup, idx);
            let idx = WindowMode::ALL.iter().position(|&m| m == d.window_mode).unwrap_or(0) as i64;
            set_popup_index(r.window_mode_popup, idx);
            let state: i64 = if d.vsync { 1 } else { 0 };
            let () = msg_send![r.vsync_checkbox, setState: state];
            let () = msg_send![r.frame_limit_field, setStringValue: ns(&d.frame_limit.to_string())];
        }
    }

    extern "C" fn save_controls(_this: &Object, _sel: Sel, _sender: id) {
        let refs = REFS.lock().unwrap();
        let Some(r) = refs.as_ref() else { return };
        let mut settings = ControlsSettings::load();
        for &(action, btn) in &r.control_labels {
            unsafe {
                let title = string_value(btn);
                let key = title.trim().to_owned();
                settings.bindings.insert(action, key);
            }
        }
        settings.save();
        eprintln!("Controls settings saved");
        notify(super::SettingsChanged::Controls);
    }

    extern "C" fn defaults_controls(_this: &Object, _sel: Sel, _sender: id) {
        let refs = REFS.lock().unwrap();
        let Some(r) = refs.as_ref() else { return };
        let d = ControlsSettings::default();
        for &(action, btn) in &r.control_labels {
            let key = d.key_for(action);
            unsafe {
                let () = msg_send![btn, setTitle: ns(&format!("  {}  ", key))];
            }
        }
    }

    // -----------------------------------------------------------------------
    // Key capture for rebinding
    // -----------------------------------------------------------------------

    // Index of the action being rebound, or -1 if none.
    static REBINDING_INDEX: Mutex<i64> = Mutex::new(-1);
    // The panel shown during rebinding.
    static REBIND_PANEL: Mutex<Option<SendId>> = Mutex::new(None);

    struct SendId(id);
    unsafe impl Send for SendId {}

    extern "C" fn click_shortcut(_this: &Object, _sel: Sel, sender: id) {
        unsafe {
            let tag: i64 = msg_send![sender, tag];
            *REBINDING_INDEX.lock().unwrap() = tag;

            // Create a small utility panel with instructions.
            let panel_class = register_key_panel_class();
            let panel: id = msg_send![panel_class, alloc];
            let frame = NSRect::new(NSPoint::new(200.0, 300.0), NSSize::new(300.0, 120.0));
            let style = NSWindowStyleMask::NSTitledWindowMask
                | NSWindowStyleMask::NSClosableWindowMask;
            let panel: id = msg_send![panel, initWithContentRect: frame
                styleMask: style
                backing: 2i64  // NSBackingStoreBuffered
                defer: NO];
            let () = msg_send![panel, setTitle: ns("Press a Key")];
            let () = msg_send![panel, setLevel: 10i64]; // NSFloatingWindowLevel

            let content: id = msg_send![panel, contentView];

            let lbl = make_label("Press a key to bind...", NSRect::new(
                NSPoint::new(20.0, 60.0),
                NSSize::new(260.0, 30.0),
            ));
            let alignment: u64 = 1; // NSTextAlignmentCenter
            let () = msg_send![lbl, setAlignment: alignment];
            let () = msg_send![content, addSubview: lbl];

            // Register the delegate class for key capture.
            let delegate_class = register_key_capture_class();
            let key_delegate: id = msg_send![delegate_class, new];
            let () = msg_send![panel, setDelegate: key_delegate];
            // Make the panel the key window to receive key events.
            // We use a custom NSPanel subclass behavior via the delegate.

            let cancel_btn = make_button("Cancel", NSRect::new(
                NSPoint::new(100.0, 15.0),
                NSSize::new(100.0, 32.0),
            ), selector("cancelRebind:"), key_delegate);
            let () = msg_send![content, addSubview: cancel_btn];

            let () = msg_send![panel, makeKeyAndOrderFront: nil];
            *REBIND_PANEL.lock().unwrap() = Some(SendId(panel));
            let () = msg_send![key_delegate, retain];
        }
    }

    extern "C" fn cancel_rebind(_this: &Object, _sel: Sel, _sender: id) {
        close_rebind_panel();
    }

    fn close_rebind_panel() {
        *REBINDING_INDEX.lock().unwrap() = -1;
        let panel = REBIND_PANEL.lock().unwrap().take();
        if let Some(p) = panel {
            unsafe {
                let () = msg_send![p.0, close];
            }
        }
    }

    fn apply_rebind(key_name: &str) {
        let idx = *REBINDING_INDEX.lock().unwrap();
        if idx < 0 || idx as usize >= Action::ALL.len() {
            return;
        }
        let refs = REFS.lock().unwrap();
        if let Some(r) = refs.as_ref() {
            if let Some(&(_, btn)) = r.control_labels.get(idx as usize) {
                unsafe {
                    let () = msg_send![btn, setTitle: ns(&format!("  {}  ", key_name))];
                }
            }
        }
        close_rebind_panel();
    }

    // Delegate for the key capture panel — receives keyDown events.
    fn register_key_capture_class() -> &'static Class {
        static ONCE: std::sync::Once = std::sync::Once::new();
        static mut CLASS: Option<&'static Class> = None;
        ONCE.call_once(|| {
            let mut decl = ClassDecl::new("ChudtendoKeyCaptureDelegate", class!(NSObject))
                .expect("failed to create key capture delegate");

            unsafe {
                decl.add_method(
                    selector("cancelRebind:"),
                    cancel_rebind as extern "C" fn(&Object, Sel, id),
                );
            }

            unsafe { CLASS = Some(decl.register()) };
        });
        unsafe { CLASS.unwrap() }
    }

    // To capture key events on the panel, we subclass NSPanel and override
    // keyDown:.  This is simpler than using a local event monitor.
    fn register_key_panel_class() -> &'static Class {
        static ONCE: std::sync::Once = std::sync::Once::new();
        static mut CLASS: Option<&'static Class> = None;
        ONCE.call_once(|| {
            let superclass = class!(NSPanel);
            let mut decl = ClassDecl::new("ChudtendoKeyPanel", superclass)
                .expect("failed to create key panel class");

            extern "C" fn panel_key_down(_this: &Object, _sel: Sel, event: id) {
                unsafe {
                    let chars: id = msg_send![event, charactersIgnoringModifiers];
                    let len: usize = msg_send![chars, length];
                    if len > 0 {
                        let cstr: *const i8 = msg_send![chars, UTF8String];
                        let s = std::ffi::CStr::from_ptr(cstr)
                            .to_string_lossy()
                            .into_owned();
                        // Try to get a readable name from the key code.
                        let key_code: u16 = msg_send![event, keyCode];
                        let name = macos_key_name(key_code, &s);
                        apply_rebind(&name);
                    }
                }
            }

            // NSPanel needs canBecomeKeyWindow to return YES for keyDown.
            extern "C" fn can_become_key(_this: &Object, _sel: Sel) -> BOOL {
                YES
            }

            unsafe {
                decl.add_method(
                    selector("keyDown:"),
                    panel_key_down as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("canBecomeKeyWindow"),
                    can_become_key as extern "C" fn(&Object, Sel) -> BOOL,
                );
            }

            unsafe { CLASS = Some(decl.register()) };
        });
        unsafe { CLASS.unwrap() }
    }

    fn macos_key_name(key_code: u16, chars: &str) -> String {
        // Map common key codes to SDL-compatible names.
        match key_code {
            0x24 => "return".to_owned(),
            0x30 => "tab".to_owned(),
            0x31 => "space".to_owned(),
            0x33 => "backspace".to_owned(),
            0x35 => "escape".to_owned(),
            0x7E => "up".to_owned(),
            0x7D => "down".to_owned(),
            0x7B => "left".to_owned(),
            0x7C => "right".to_owned(),
            0x32 => "`".to_owned(),
            0x7A => "f1".to_owned(),
            0x78 => "f2".to_owned(),
            0x63 => "f3".to_owned(),
            0x76 => "f4".to_owned(),
            0x60 => "f5".to_owned(),
            0x61 => "f6".to_owned(),
            0x62 => "f7".to_owned(),
            0x64 => "f8".to_owned(),
            0x65 => "f9".to_owned(),
            0x6D => "f10".to_owned(),
            0x67 => "f11".to_owned(),
            0x6F => "f12".to_owned(),
            0x75 => "delete".to_owned(),
            0x73 => "home".to_owned(),
            0x77 => "end".to_owned(),
            0x74 => "pageup".to_owned(),
            0x79 => "pagedown".to_owned(),
            _ => {
                // Use the character itself for printable keys.
                let lower = chars.to_lowercase();
                if lower.is_empty() {
                    format!("keycode_{key_code}")
                } else {
                    lower
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Preferences delegate class
    // -----------------------------------------------------------------------

    fn register_prefs_delegate_class() -> &'static Class {
        static ONCE: std::sync::Once = std::sync::Once::new();
        static mut CLASS: Option<&'static Class> = None;
        ONCE.call_once(|| {
            let mut decl = ClassDecl::new("ChudtendoPrefsDelegate", class!(NSObject))
                .expect("failed to create prefs delegate class");

            unsafe {
                decl.add_method(
                    selector("saveEmulation:"),
                    save_emulation as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("defaultsEmulation:"),
                    defaults_emulation as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("saveDisplay:"),
                    save_display as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("defaultsDisplay:"),
                    defaults_display as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("saveControls:"),
                    save_controls as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("defaultsControls:"),
                    defaults_controls as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector("clickShortcut:"),
                    click_shortcut as extern "C" fn(&Object, Sel, id),
                );
            }

            unsafe { CLASS = Some(decl.register()) };
        });
        unsafe { CLASS.unwrap() }
    }

    // -----------------------------------------------------------------------
    // Public entry point
    // -----------------------------------------------------------------------

    pub fn open_preferences_window() {
        unsafe {
            let _pool = NSAutoreleasePool::new(nil);

            // Initialize refs.
            *REFS.lock().unwrap() = Some(Refs {
                ff_speed_popup: nil,
                rewind_popup: nil,
                boot_rom_popup: nil,
                scale_popup: nil,
                window_mode_popup: nil,
                vsync_checkbox: nil,
                frame_limit_field: nil,
                control_labels: Vec::new(),
            });

            let delegate_class = register_prefs_delegate_class();
            let delegate: id = msg_send![delegate_class, new];
            let () = msg_send![delegate, retain];

            // Create the window.
            let frame = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(WIN_W, WIN_H));
            let style = NSWindowStyleMask::NSTitledWindowMask
                | NSWindowStyleMask::NSClosableWindowMask
                | NSWindowStyleMask::NSMiniaturizableWindowMask;

            let window: id = msg_send![class!(NSWindow), alloc];
            let window: id = msg_send![window, initWithContentRect: frame
                styleMask: style
                backing: 2i64  // NSBackingStoreBuffered
                defer: NO];
            let () = msg_send![window, setTitle: ns("Settings")];
            let () = msg_send![window, center];

            // Create tab view.
            let content: id = msg_send![window, contentView];
            let content_frame: NSRect = msg_send![content, frame];

            let tab_view: id = msg_send![class!(NSTabView), alloc];
            let tab_view: id = msg_send![tab_view, initWithFrame: content_frame];

            // Tab 1: Emulation
            let tab1: id = msg_send![class!(NSTabViewItem), alloc];
            let tab1: id = msg_send![tab1, initWithIdentifier: ns("emulation")];
            let () = msg_send![tab1, setLabel: ns("Emulation")];
            let emu_view = build_emulation_tab(delegate);
            let () = msg_send![tab1, setView: emu_view];
            let () = msg_send![tab_view, addTabViewItem: tab1];

            // Tab 2: Display
            let tab2: id = msg_send![class!(NSTabViewItem), alloc];
            let tab2: id = msg_send![tab2, initWithIdentifier: ns("display")];
            let () = msg_send![tab2, setLabel: ns("Display")];
            let display_view = build_display_tab(delegate);
            let () = msg_send![tab2, setView: display_view];
            let () = msg_send![tab_view, addTabViewItem: tab2];

            // Tab 3: Controls
            let tab3: id = msg_send![class!(NSTabViewItem), alloc];
            let tab3: id = msg_send![tab3, initWithIdentifier: ns("controls")];
            let () = msg_send![tab3, setLabel: ns("Controls")];
            let controls_view = build_controls_tab(delegate);
            let () = msg_send![tab3, setView: controls_view];
            let () = msg_send![tab_view, addTabViewItem: tab3];

            let () = msg_send![content, addSubview: tab_view];

            let () = msg_send![window, makeKeyAndOrderFront: nil];
            // Prevent deallocation.
            let () = msg_send![window, retain];
        }
    }
}

// ---------------------------------------------------------------------------
// Windows — Win32 preferences dialog
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::Mutex;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::Controls::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    use crate::settings::{
        Action, ControlsSettings, DisplaySettings, EmulationSettings, WindowMode,
    };

    pub static SETTINGS_SENDER: Mutex<
        Option<std::sync::mpsc::SyncSender<super::SettingsChanged>>,
    > = Mutex::new(None);

    fn notify(kind: super::SettingsChanged) {
        if let Some(ref tx) = *SETTINGS_SENDER.lock().unwrap() {
            let _ = tx.try_send(kind);
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // Control IDs for the preferences dialog.
    const ID_TAB: i32 = 1000;
    const ID_FF_SPEED: i32 = 1100;
    const ID_REWIND_BUF: i32 = 1101;
    const ID_BOOT_ROM: i32 = 1102;
    const ID_SAVE_EMU: i32 = 1103;
    const ID_DEFAULTS_EMU: i32 = 1104;
    const ID_SCALE: i32 = 1200;
    const ID_WIN_MODE: i32 = 1201;
    const ID_VSYNC: i32 = 1202;
    const ID_FRAME_LIMIT: i32 = 1203;
    const ID_SAVE_DISPLAY: i32 = 1204;
    const ID_DEFAULTS_DISPLAY: i32 = 1205;
    const ID_SAVE_CONTROLS: i32 = 1300;
    const ID_DEFAULTS_CONTROLS: i32 = 1301;
    const ID_CONTROLS_LIST: i32 = 1302;

    const FF_SPEEDS: &[(&str, f32)] = &[
        ("2x", 2.0), ("4x", 4.0), ("8x", 8.0), ("Uncapped", 0.0),
    ];
    const REWIND_SIZES: &[(&str, u32)] = &[
        ("10 seconds", 10), ("30 seconds", 30), ("60 seconds", 60), ("120 seconds", 120),
    ];

    static PREFS_HWND: Mutex<Option<isize>> = Mutex::new(None);

    unsafe fn create_label(parent: HWND, text: &str, x: i32, y: i32, w: i32, h: i32) {
        let cls = wide("STATIC");
        let txt = wide(text);
        CreateWindowExW(
            WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(txt.as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_RIGHT as u32),
            x, y, w, h, parent, HMENU(0 as _), None, None,
        );
    }

    unsafe fn create_combo(parent: HWND, id: i32, items: &[&str], x: i32, y: i32, w: i32, h: i32) -> HWND {
        let cls = wide("COMBOBOX");
        let combo = CreateWindowExW(
            WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32),
            x, y, w, h, parent, HMENU(id as _), None, None,
        ).unwrap_or(HWND(std::ptr::null_mut()));

        for item in items {
            let witem = wide(item);
            SendMessageW(combo, CB_ADDSTRING, WPARAM(0), LPARAM(witem.as_ptr() as isize));
        }
        combo
    }

    unsafe fn create_button(parent: HWND, id: i32, text: &str, x: i32, y: i32, w: i32, h: i32) {
        let cls = wide("BUTTON");
        let txt = wide(text);
        CreateWindowExW(
            WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(txt.as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            x, y, w, h, parent, HMENU(id as _), None, None,
        );
    }

    unsafe fn create_checkbox(parent: HWND, id: i32, text: &str, x: i32, y: i32, w: i32, h: i32) -> HWND {
        let cls = wide("BUTTON");
        let txt = wide(text);
        CreateWindowExW(
            WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(txt.as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            x, y, w, h, parent, HMENU(id as _), None, None,
        ).unwrap_or(HWND(std::ptr::null_mut()))
    }

    unsafe fn create_edit(parent: HWND, id: i32, text: &str, x: i32, y: i32, w: i32, h: i32) -> HWND {
        let cls = wide("EDIT");
        let txt = wide(text);
        CreateWindowExW(
            WS_EX_CLIENTEDGE, PCWSTR(cls.as_ptr()), PCWSTR(txt.as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            x, y, w, h, parent, HMENU(id as _), None, None,
        ).unwrap_or(HWND(std::ptr::null_mut()))
    }

    unsafe fn combo_select(hwnd: HWND, idx: i32) {
        SendMessageW(hwnd, CB_SETCURSEL, WPARAM(idx as usize), LPARAM(0));
    }

    unsafe fn combo_index(hwnd: HWND) -> i32 {
        SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32
    }

    unsafe fn get_dlg_item(hwnd: HWND, id: i32) -> HWND {
        GetDlgItem(hwnd, id).unwrap_or(HWND(std::ptr::null_mut()))
    }

    unsafe fn get_window_text(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }

    unsafe fn handle_command(hwnd: HWND, cmd: i32) {
        match cmd {
            x if x == ID_SAVE_EMU => {
                let ff_idx = combo_index(get_dlg_item(hwnd, ID_FF_SPEED)) as usize;
                let rw_idx = combo_index(get_dlg_item(hwnd, ID_REWIND_BUF)) as usize;
                let br_idx = combo_index(get_dlg_item(hwnd, ID_BOOT_ROM)) as usize;
                let settings = EmulationSettings {
                    ff_speed: FF_SPEEDS.get(ff_idx).map(|&(_, v)| v).unwrap_or(0.0),
                    rewind_buffer_seconds: REWIND_SIZES.get(rw_idx).map(|&(_, v)| v).unwrap_or(30),
                    boot_rom: if br_idx == 1 { "dmg" } else { "cgb" }.to_owned(),
                };
                settings.save();
                notify(super::SettingsChanged::Emulation);
            }
            x if x == ID_DEFAULTS_EMU => {
                let d = EmulationSettings::default();
                let idx = FF_SPEEDS.iter().position(|&(_, v)| v == d.ff_speed).unwrap_or(3);
                combo_select(get_dlg_item(hwnd, ID_FF_SPEED), idx as i32);
                let idx = REWIND_SIZES.iter().position(|&(_, v)| v == d.rewind_buffer_seconds).unwrap_or(1);
                combo_select(get_dlg_item(hwnd, ID_REWIND_BUF), idx as i32);
                combo_select(get_dlg_item(hwnd, ID_BOOT_ROM), 0);
            }
            x if x == ID_SAVE_DISPLAY => {
                let scale_idx = combo_index(get_dlg_item(hwnd, ID_SCALE)) as usize;
                let mode_idx = combo_index(get_dlg_item(hwnd, ID_WIN_MODE)) as usize;
                let vsync_hwnd = get_dlg_item(hwnd, ID_VSYNC);
                let vsync = SendMessageW(vsync_hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 != 0;
                let fl_text = get_window_text(get_dlg_item(hwnd, ID_FRAME_LIMIT));
                let settings = DisplaySettings {
                    window_scale: (scale_idx as u32 + 1).min(8),
                    window_mode: WindowMode::ALL.get(mode_idx).copied().unwrap_or(WindowMode::Windowed),
                    vsync,
                    frame_limit: fl_text.parse().unwrap_or(0),
                };
                settings.save();
                notify(super::SettingsChanged::Display);
            }
            x if x == ID_DEFAULTS_DISPLAY => {
                let d = DisplaySettings::default();
                combo_select(get_dlg_item(hwnd, ID_SCALE), (d.window_scale - 1) as i32);
                combo_select(get_dlg_item(hwnd, ID_WIN_MODE), 0);
                let state = if d.vsync { BST_CHECKED } else { BST_UNCHECKED };
                SendMessageW(get_dlg_item(hwnd, ID_VSYNC), BM_SETCHECK, WPARAM(state.0 as usize), LPARAM(0));
                let txt = wide(&d.frame_limit.to_string());
                SetWindowTextW(get_dlg_item(hwnd, ID_FRAME_LIMIT), PCWSTR(txt.as_ptr()));
            }
            x if x == ID_SAVE_CONTROLS => {
                // TODO: read from list view and save
                let controls = ControlsSettings::load();
                controls.save();
                notify(super::SettingsChanged::Controls);
            }
            x if x == ID_DEFAULTS_CONTROLS => {
                // TODO: reset list view to defaults
            }
            _ => {}
        }
    }

    unsafe extern "system" fn prefs_wndproc(
        hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COMMAND => {
                let cmd = (wparam.0 & 0xFFFF) as i32;
                handle_command(hwnd, cmd);
                LRESULT(0)
            }
            WM_DESTROY => {
                *PREFS_HWND.lock().unwrap() = None;
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    pub fn open_preferences_window() {
        // Don't open multiple instances.
        if PREFS_HWND.lock().unwrap().is_some() {
            return;
        }

        unsafe {
            let class_name = wide("ChudtendoPrefs");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(prefs_wndproc),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as _),
                ..Default::default()
            };
            RegisterClassW(&wc);

            let title = wide("Settings");
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT, CW_USEDEFAULT, 520, 460,
                HWND(std::ptr::null_mut()), HMENU(std::ptr::null_mut()), None, None,
            ).unwrap_or(HWND(std::ptr::null_mut()));

            if hwnd.0.is_null() {
                eprintln!("Failed to create preferences window");
                return;
            }

            *PREFS_HWND.lock().unwrap() = Some(hwnd.0 as isize);

            // Create tab control.
            let tab_cls = wide("SysTabControl32");
            let tab_hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0), PCWSTR(tab_cls.as_ptr()), PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(TCS_TABS as u32),
                5, 5, 500, 410,
                hwnd, HMENU(ID_TAB as _), None, None,
            ).unwrap_or(HWND(std::ptr::null_mut()));

            // Add tabs.
            for (i, name) in ["Emulation", "Display", "Controls"].iter().enumerate() {
                let wname = wide(name);
                let mut item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: windows::core::PWSTR(wname.as_ptr() as *mut u16),
                    ..Default::default()
                };
                SendMessageW(
                    tab_hwnd,
                    TCM_INSERTITEMW,
                    WPARAM(i),
                    LPARAM(&mut item as *mut _ as isize),
                );
            }

            // --- Emulation tab controls ---
            let ox = 20;
            let oy = 40;
            create_label(hwnd, "Fast Forward Speed:", ox, oy, 160, 22);
            let ff = create_combo(
                hwnd, ID_FF_SPEED,
                &FF_SPEEDS.iter().map(|&(n, _)| n).collect::<Vec<_>>(),
                ox + 170, oy, 180, 200,
            );
            let emu = EmulationSettings::load();
            combo_select(ff, FF_SPEEDS.iter().position(|&(_, v)| v == emu.ff_speed).unwrap_or(3) as i32);

            create_label(hwnd, "Rewind Buffer:", ox, oy + 30, 160, 22);
            let rw = create_combo(
                hwnd, ID_REWIND_BUF,
                &REWIND_SIZES.iter().map(|&(n, _)| n).collect::<Vec<_>>(),
                ox + 170, oy + 30, 180, 200,
            );
            combo_select(rw, REWIND_SIZES.iter().position(|&(_, v)| v == emu.rewind_buffer_seconds).unwrap_or(1) as i32);

            create_label(hwnd, "Boot ROM:", ox, oy + 60, 160, 22);
            let br = create_combo(
                hwnd, ID_BOOT_ROM, &["CGB (Color)", "DMG (Original)"],
                ox + 170, oy + 60, 180, 200,
            );
            combo_select(br, if emu.boot_rom == "dmg" { 1 } else { 0 });

            create_button(hwnd, ID_SAVE_EMU, "Save", 390, 380, 90, 30);
            create_button(hwnd, ID_DEFAULTS_EMU, "Defaults", 290, 380, 90, 30);

            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// Qt backend — used on Linux, or any platform with `--features qt`
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "linux", feature = "qt"))]
mod platform {
    use std::sync::Mutex;

    use crate::settings::{
        Action, ControlsSettings, DisplaySettings, EmulationSettings, WindowMode,
    };

    pub static SETTINGS_SENDER: Mutex<
        Option<std::sync::mpsc::SyncSender<super::SettingsChanged>>,
    > = Mutex::new(None);

    fn notify(kind: super::SettingsChanged) {
        if let Some(ref tx) = *SETTINGS_SENDER.lock().unwrap() {
            let _ = tx.try_send(kind);
        }
    }

    pub fn open_preferences_window() {
        let emu = EmulationSettings::load();
        let display = DisplaySettings::load();
        let controls = ControlsSettings::load();

        // Build key_bindings string: "key_up=up;key_down=down;..."
        let bindings_str: String = Action::ALL
            .iter()
            .filter_map(|&action| {
                let key = controls.key_for(action);
                if key.is_empty() {
                    None
                } else {
                    Some(format!("{}={}", action.config_key(), key))
                }
            })
            .collect::<Vec<_>>()
            .join(";");

        let boot_rom_c = std::ffi::CString::new(emu.boot_rom.as_str()).unwrap_or_default();
        let bindings_c = std::ffi::CString::new(bindings_str.as_str()).unwrap_or_default();

        let mode_idx = WindowMode::ALL
            .iter()
            .position(|&m| m == display.window_mode)
            .unwrap_or(0);

        unsafe {
            crate::qt_ffi::qt_open_preferences(
                emu.ff_speed,
                emu.rewind_buffer_seconds as i32,
                boot_rom_c.as_ptr(),
                display.window_scale as i32,
                mode_idx as i32,
                if display.vsync { 1 } else { 0 },
                display.frame_limit as i32,
                bindings_c.as_ptr(),
            );
        }

        // Start a polling thread for preferences save notifications.
        // The Qt action codes -1, -2, -3 correspond to settings changes.
        // The main Qt event pump in menu.rs handles these too, but we
        // also need to read back widget values and save them.
        std::thread::Builder::new()
            .name("qt-prefs-poll".to_owned())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));

                    let code = unsafe { crate::qt_ffi::qt_poll_action() };
                    match code {
                        -1 => {
                            // Save emulation settings from Qt widgets.
                            let settings = unsafe {
                                EmulationSettings {
                                    ff_speed: crate::qt_ffi::qt_prefs_ff_speed(),
                                    rewind_buffer_seconds: crate::qt_ffi::qt_prefs_rewind_secs() as u32,
                                    boot_rom: if crate::qt_ffi::qt_prefs_boot_rom_is_dmg() != 0 {
                                        "dmg".to_owned()
                                    } else {
                                        "cgb".to_owned()
                                    },
                                }
                            };
                            settings.save();
                            notify(super::SettingsChanged::Emulation);
                            eprintln!("Emulation settings saved (Qt)");
                        }
                        -2 => {
                            let settings = unsafe {
                                DisplaySettings {
                                    window_scale: crate::qt_ffi::qt_prefs_scale() as u32,
                                    window_mode: WindowMode::ALL
                                        .get(crate::qt_ffi::qt_prefs_window_mode() as usize)
                                        .copied()
                                        .unwrap_or(WindowMode::Windowed),
                                    vsync: crate::qt_ffi::qt_prefs_vsync() != 0,
                                    frame_limit: crate::qt_ffi::qt_prefs_frame_limit() as u32,
                                }
                            };
                            settings.save();
                            notify(super::SettingsChanged::Display);
                            eprintln!("Display settings saved (Qt)");
                        }
                        -3 => {
                            let bindings_ptr = unsafe { crate::qt_ffi::qt_prefs_key_bindings() };
                            if !bindings_ptr.is_null() {
                                let bindings_str = unsafe {
                                    std::ffi::CStr::from_ptr(bindings_ptr)
                                        .to_string_lossy()
                                        .into_owned()
                                };
                                unsafe { crate::qt_ffi::qt_free_string(bindings_ptr) };

                                let mut settings = ControlsSettings::load();
                                for pair in bindings_str.split(';') {
                                    let mut kv = pair.splitn(2, '=');
                                    if let (Some(config_key), Some(value)) = (kv.next(), kv.next()) {
                                        // Find matching action by config_key.
                                        for &action in &Action::ALL {
                                            if action.config_key() == config_key {
                                                settings.bindings.insert(action, value.to_owned());
                                                break;
                                            }
                                        }
                                    }
                                }
                                settings.save();
                                notify(super::SettingsChanged::Controls);
                                eprintln!("Controls settings saved (Qt)");
                            }
                        }
                        0 => {} // no action
                        _ => {} // handled by menu event pump
                    }
                }
            })
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Fallback
// ---------------------------------------------------------------------------

#[cfg(not(any(
    all(target_os = "macos", not(feature = "qt")),
    target_os = "windows",
    target_os = "linux",
    feature = "qt",
)))]
mod platform {
    pub fn open_preferences_window() {
        eprintln!("Preferences window not available on this platform");
    }
}

pub use platform::open_preferences_window;
