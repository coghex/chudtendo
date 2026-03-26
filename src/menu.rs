#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    LoadRom,
    Quit,
    Pause,
    Reset,
    SaveState(u8),
    LoadState(u8),
    ToggleFastForward,
    ToggleRewind,
}

/// Description of a single save-state slot, used to populate menu labels.
#[derive(Clone, Debug)]
pub struct SlotInfo {
    pub slot: u8,
    pub timestamp: Option<String>,
}

/// Callback the menu system uses to query current slot status right before
/// the submenu opens.  The app supplies this; the menu module calls it.
pub type SlotInfoFn = Box<dyn Fn() -> Vec<SlotInfo> + Send>;

#[cfg(target_os = "macos")]
mod platform {
    use super::{MenuAction, SlotInfoFn};

    use cocoa::appkit::{
        NSApp, NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem,
    };
    use cocoa::base::{id, nil, selector};
    use cocoa::foundation::{NSAutoreleasePool, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};

    use std::sync::mpsc;

    use std::sync::Mutex;

    /// Wrapper for ObjC id pointers so they can live in a Mutex.
    /// Safety: these are only accessed from the main (AppKit) thread.
    struct SendId(id);
    unsafe impl Send for SendId {}

    static MENU_SENDER: Mutex<Option<mpsc::SyncSender<MenuAction>>> = Mutex::new(None);
    static SLOT_INFO_FN: Mutex<Option<SlotInfoFn>> = Mutex::new(None);
    // Store references to save/load menu items so we can update their titles.
    // Only accessed from the main (AppKit) thread.
    static SAVE_ITEMS: Mutex<[SendId; 8]> = Mutex::new([
        SendId(0 as id), SendId(0 as id), SendId(0 as id), SendId(0 as id),
        SendId(0 as id), SendId(0 as id), SendId(0 as id), SendId(0 as id),
    ]);
    static LOAD_ITEMS: Mutex<[SendId; 8]> = Mutex::new([
        SendId(0 as id), SendId(0 as id), SendId(0 as id), SendId(0 as id),
        SendId(0 as id), SendId(0 as id), SendId(0 as id), SendId(0 as id),
    ]);

    fn send_action(action: MenuAction) {
        if let Some(ref sender) = *MENU_SENDER.lock().unwrap() {
            let _ = sender.try_send(action);
        }
    }

    // --- Action handlers ---

    extern "C" fn action_load_rom(_this: &Object, _sel: Sel, _sender: id) {
        send_action(MenuAction::LoadRom);
    }

    extern "C" fn action_quit(_this: &Object, _sel: Sel, _sender: id) {
        send_action(MenuAction::Quit);
    }

    extern "C" fn action_pause(_this: &Object, _sel: Sel, _sender: id) {
        send_action(MenuAction::Pause);
    }

    extern "C" fn action_reset(_this: &Object, _sel: Sel, _sender: id) {
        send_action(MenuAction::Reset);
    }

    extern "C" fn action_fast_forward(_this: &Object, _sel: Sel, _sender: id) {
        send_action(MenuAction::ToggleFastForward);
    }

    extern "C" fn action_rewind(_this: &Object, _sel: Sel, _sender: id) {
        send_action(MenuAction::ToggleRewind);
    }

    macro_rules! save_slot_handler {
        ($name:ident, $slot:expr) => {
            extern "C" fn $name(_this: &Object, _sel: Sel, _sender: id) {
                send_action(MenuAction::SaveState($slot));
            }
        };
    }

    macro_rules! load_slot_handler {
        ($name:ident, $slot:expr) => {
            extern "C" fn $name(_this: &Object, _sel: Sel, _sender: id) {
                send_action(MenuAction::LoadState($slot));
            }
        };
    }

    save_slot_handler!(action_save_1, 1);
    save_slot_handler!(action_save_2, 2);
    save_slot_handler!(action_save_3, 3);
    save_slot_handler!(action_save_4, 4);
    save_slot_handler!(action_save_5, 5);
    save_slot_handler!(action_save_6, 6);
    save_slot_handler!(action_save_7, 7);
    save_slot_handler!(action_save_8, 8);

    load_slot_handler!(action_load_1, 1);
    load_slot_handler!(action_load_2, 2);
    load_slot_handler!(action_load_3, 3);
    load_slot_handler!(action_load_4, 4);
    load_slot_handler!(action_load_5, 5);
    load_slot_handler!(action_load_6, 6);
    load_slot_handler!(action_load_7, 7);
    load_slot_handler!(action_load_8, 8);

    const SAVE_HANDLERS: [extern "C" fn(&Object, Sel, id); 8] = [
        action_save_1, action_save_2, action_save_3, action_save_4,
        action_save_5, action_save_6, action_save_7, action_save_8,
    ];
    const LOAD_HANDLERS: [extern "C" fn(&Object, Sel, id); 8] = [
        action_load_1, action_load_2, action_load_3, action_load_4,
        action_load_5, action_load_6, action_load_7, action_load_8,
    ];

    const SAVE_SELECTORS: [&str; 8] = [
        "saveSlot1:", "saveSlot2:", "saveSlot3:", "saveSlot4:",
        "saveSlot5:", "saveSlot6:", "saveSlot7:", "saveSlot8:",
    ];
    const LOAD_SELECTORS: [&str; 8] = [
        "loadSlot1:", "loadSlot2:", "loadSlot3:", "loadSlot4:",
        "loadSlot5:", "loadSlot6:", "loadSlot7:", "loadSlot8:",
    ];

    // F1 through F8 key equivalents (NSF1FunctionKey = 0xF704, etc.)
    fn fkey_char(n: u8) -> String {
        let code = 0xF704u32 + (n - 1) as u32;
        char::from_u32(code).map_or(String::new(), |c| c.to_string())
    }

    /// Called by AppKit when a submenu is about to open — refresh slot labels.
    extern "C" fn menu_needs_update(_this: &Object, _sel: Sel, menu: id) {
        unsafe {
            let title: id = msg_send![menu, title];
            let title_len: usize = msg_send![title, length];
            if title_len == 0 {
                return;
            }

            let infos = {
                let guard = SLOT_INFO_FN.lock().unwrap();
                match &*guard {
                    Some(f) => f(),
                    None => return,
                }
            };

            // Figure out if this is the save or load submenu by checking
            // the first item's action selector.
            let count: i64 = msg_send![menu, numberOfItems];
            if count == 0 {
                return;
            }
            let first_item: id = msg_send![menu, itemAtIndex: 0i64];
            let first_action: Sel = msg_send![first_item, action];
            let is_save = first_action == selector("saveSlot1:");

            let items_guard;
            let items_guard2;
            let items: &[SendId; 8] = if is_save {
                items_guard = SAVE_ITEMS.lock().unwrap();
                &*items_guard
            } else {
                items_guard2 = LOAD_ITEMS.lock().unwrap();
                &*items_guard2
            };

            for info in &infos {
                let idx = (info.slot - 1) as usize;
                if idx >= 8 {
                    continue;
                }
                let item = items[idx].0;
                if item == nil {
                    continue;
                }
                let label = match &info.timestamp {
                    Some(ts) => format!("Slot {} - {}", info.slot, ts),
                    None => format!("Slot {} - Empty", info.slot),
                };
                let ns_title = NSString::alloc(nil).init_str(&label);
                let () = msg_send![item, setTitle: ns_title];
            }
        }
    }

    fn register_delegate_class() -> &'static Class {
        let mut decl = ClassDecl::new("ChudtendoMenuDelegate", class!(NSObject))
            .expect("failed to create menu delegate class");

        unsafe {
            decl.add_method(
                selector("loadRom:"),
                action_load_rom as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                selector("quitApp:"),
                action_quit as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                selector("togglePause:"),
                action_pause as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                selector("resetEmulator:"),
                action_reset as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                selector("toggleFastForward:"),
                action_fast_forward as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                selector("toggleRewind:"),
                action_rewind as extern "C" fn(&Object, Sel, id),
            );

            for i in 0..8 {
                decl.add_method(
                    selector(SAVE_SELECTORS[i]),
                    SAVE_HANDLERS[i] as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    selector(LOAD_SELECTORS[i]),
                    LOAD_HANDLERS[i] as extern "C" fn(&Object, Sel, id),
                );
            }

            decl.add_method(
                selector("menuNeedsUpdate:"),
                menu_needs_update as extern "C" fn(&Object, Sel, id),
            );
        }

        decl.register()
    }

    unsafe fn make_menu_item(
        title: &str,
        action: Sel,
        key: &str,
        modifier: NSEventModifierFlags,
        target: id,
    ) -> id {
        let item = NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(
            NSString::alloc(nil).init_str(title),
            action,
            NSString::alloc(nil).init_str(key),
        );
        let () = msg_send![item, setKeyEquivalentModifierMask: modifier];
        let () = msg_send![item, setTarget: target];
        item
    }

    unsafe fn build_slot_submenu(
        title: &str,
        selectors: &[&str; 8],
        is_save: bool,
        delegate: id,
    ) -> id {
        let menu = NSMenu::new(nil).autorelease();
        let () = msg_send![menu, setTitle: NSString::alloc(nil).init_str(title)];
        let () = msg_send![menu, setDelegate: delegate];

        // Use function key modifiers: none for save, shift for load.
        let modifier = if is_save {
            NSEventModifierFlags::NSFunctionKeyMask
        } else {
            NSEventModifierFlags::from_bits_truncate(
                NSEventModifierFlags::NSFunctionKeyMask.bits()
                    | NSEventModifierFlags::NSShiftKeyMask.bits(),
            )
        };

        for i in 0..8 {
            let slot = (i + 1) as u8;
            let label = format!("Slot {} - Empty", slot);
            let key_equiv = fkey_char(slot);
            let item = make_menu_item(
                &label,
                selector(selectors[i]),
                &key_equiv,
                modifier,
                delegate,
            );
            menu.addItem_(item);

            if is_save {
                SAVE_ITEMS.lock().unwrap()[i] = SendId(item);
            } else {
                LOAD_ITEMS.lock().unwrap()[i] = SendId(item);
            }
        }

        menu
    }

    pub fn install_menu_bar(slot_info_fn: SlotInfoFn) -> mpsc::Receiver<MenuAction> {
        let (sender, receiver) = mpsc::sync_channel(16);

        unsafe {
            *MENU_SENDER.lock().unwrap() = Some(sender);
            *SLOT_INFO_FN.lock().unwrap() = Some(slot_info_fn);

            let _pool = NSAutoreleasePool::new(nil);
            let app = NSApp();

            let delegate_class = register_delegate_class();
            let delegate: id = msg_send![delegate_class, new];

            // --- File menu ---
            let file_menu = NSMenu::new(nil).autorelease();
            let () = msg_send![file_menu, setTitle: NSString::alloc(nil).init_str("File")];

            let load_item = make_menu_item(
                "Load ROM\u{2026}",
                selector("loadRom:"),
                "o",
                NSEventModifierFlags::NSCommandKeyMask,
                delegate,
            );
            file_menu.addItem_(load_item);

            file_menu.addItem_(NSMenuItem::separatorItem(nil));

            let quit_item = make_menu_item(
                "Quit",
                selector("quitApp:"),
                "q",
                NSEventModifierFlags::NSCommandKeyMask,
                delegate,
            );
            file_menu.addItem_(quit_item);

            let file_bar_item = NSMenuItem::new(nil).autorelease();
            file_bar_item.setSubmenu_(file_menu);

            // --- Emulation menu ---
            let emu_menu = NSMenu::new(nil).autorelease();
            let () = msg_send![emu_menu, setTitle: NSString::alloc(nil).init_str("Emulation")];

            let pause_item = make_menu_item(
                "Pause",
                selector("togglePause:"),
                "p",
                NSEventModifierFlags::NSCommandKeyMask,
                delegate,
            );
            emu_menu.addItem_(pause_item);

            let reset_item = make_menu_item(
                "Reset",
                selector("resetEmulator:"),
                "r",
                NSEventModifierFlags::NSCommandKeyMask,
                delegate,
            );
            emu_menu.addItem_(reset_item);

            let ff_item = make_menu_item(
                "Fast Forward",
                selector("toggleFastForward:"),
                "f",
                NSEventModifierFlags::NSCommandKeyMask,
                delegate,
            );
            emu_menu.addItem_(ff_item);

            let rewind_item = make_menu_item(
                "Rewind",
                selector("toggleRewind:"),
                "w",
                NSEventModifierFlags::NSCommandKeyMask,
                delegate,
            );
            emu_menu.addItem_(rewind_item);

            emu_menu.addItem_(NSMenuItem::separatorItem(nil));

            // Save State submenu
            let save_submenu = build_slot_submenu(
                "Save State",
                &SAVE_SELECTORS,
                true,
                delegate,
            );
            let save_bar_item = NSMenuItem::new(nil).autorelease();
            let () = msg_send![save_bar_item, setTitle: NSString::alloc(nil).init_str("Save State")];
            save_bar_item.setSubmenu_(save_submenu);
            emu_menu.addItem_(save_bar_item);

            // Load State submenu
            let load_submenu = build_slot_submenu(
                "Load State",
                &LOAD_SELECTORS,
                false,
                delegate,
            );
            let load_bar_item = NSMenuItem::new(nil).autorelease();
            let () = msg_send![load_bar_item, setTitle: NSString::alloc(nil).init_str("Load State")];
            load_bar_item.setSubmenu_(load_submenu);
            emu_menu.addItem_(load_bar_item);

            let emu_bar_item = NSMenuItem::new(nil).autorelease();
            emu_bar_item.setSubmenu_(emu_menu);

            // Get the existing menu bar (SDL2 creates one) and append our menus.
            let main_menu: id = msg_send![app, mainMenu];
            if main_menu != nil {
                // Insert File after the app menu (index 1), Emulation after that.
                let count: i64 = msg_send![main_menu, numberOfItems];
                let insert_idx = if count > 0 { 1 } else { 0 };
                let () = msg_send![main_menu, insertItem: file_bar_item atIndex: insert_idx];
                let () = msg_send![main_menu, insertItem: emu_bar_item atIndex: insert_idx + 1];
            } else {
                let menu_bar = NSMenu::new(nil).autorelease();
                menu_bar.addItem_(file_bar_item);
                menu_bar.addItem_(emu_bar_item);
                app.setMainMenu_(menu_bar);
            }

            // Prevent the delegate from being deallocated.
            let () = msg_send![delegate, retain];
        }

        receiver
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{MenuAction, SlotInfoFn};
    use std::sync::mpsc;

    pub fn install_menu_bar(_slot_info_fn: SlotInfoFn) -> mpsc::Receiver<MenuAction> {
        let (_sender, receiver) = mpsc::sync_channel(16);
        // No native menu bar on this platform yet (Phase 6).
        receiver
    }
}

pub use platform::install_menu_bar;
