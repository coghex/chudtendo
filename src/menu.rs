#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    LoadRom,
    Quit,
    Pause,
    Reset,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::MenuAction;

    use cocoa::appkit::{
        NSApp, NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem,
    };
    use cocoa::base::{id, nil, selector};
    use cocoa::foundation::{NSAutoreleasePool, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};

    use std::sync::mpsc;

    static mut MENU_SENDER: Option<mpsc::SyncSender<MenuAction>> = None;

    fn send_action(action: MenuAction) {
        unsafe {
            if let Some(ref sender) = MENU_SENDER {
                let _ = sender.try_send(action);
            }
        }
    }

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

    pub fn install_menu_bar() -> mpsc::Receiver<MenuAction> {
        let (sender, receiver) = mpsc::sync_channel(16);

        unsafe {
            MENU_SENDER = Some(sender);

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
    use super::MenuAction;
    use std::sync::mpsc;

    pub fn install_menu_bar() -> mpsc::Receiver<MenuAction> {
        let (_sender, receiver) = mpsc::sync_channel(16);
        // No native menu bar on this platform yet (Phase 6).
        receiver
    }
}

pub use platform::install_menu_bar;
