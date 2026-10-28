//! AppKit menu actions are queued for application state after drawing.
use std::sync::{Arc, Mutex};
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Show,
    Quit,
    Save,
    Feed,
    Inbox,
    Archive,
    Feeds,
    Magazines,
    WhatsApp,
    ImportExport,
    Search,
    Cut,
    Copy,
    Paste,
    SelectAll,
    DisplayPreferencesChanged,
}
static QUEUE: Mutex<Vec<Command>> = Mutex::new(Vec::new());
static WAKE: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);
fn push(command: Command) {
    if let Ok(mut queue) = QUEUE.lock()
        && queue.len() < 32
    {
        queue.push(command);
    }
    if let Ok(wake) = WAKE.lock()
        && let Some(wake) = wake.as_ref()
    {
        wake();
    }
}
pub fn drain() -> Vec<Command> {
    QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}
/// A key NSWindow can belong to an inactive application. Winit's Focus
/// command skips activation in that state, so an explicit Show must activate
/// the application as well as reveal its window.
pub fn activate() {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
        #[allow(deprecated)]
        objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}
pub fn reduce_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
pub fn install(wake: Arc<dyn Fn() + Send + Sync>) {
    *WAKE.lock().expect("Menu wake lock") = Some(wake);
    macos::install();
}
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::{
        MainThreadOnly, define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Bool, MethodImplementation, Sel},
        sel,
    };
    use objc2_app_kit::{
        NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSWorkspace,
        NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
    };
    use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSString};
    use std::cell::OnceCell;
    thread_local! { static HANDLER: OnceCell<Retained<MenuHandler>> = const { OnceCell::new() }; }
    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind=MainThreadOnly]
        #[name="ReadingListMenuHandler"]
        struct MenuHandler;
        impl MenuHandler {
            #[unsafe(method(displayOptionsChanged:))]
            fn display_options_changed(&self, _notification: &NSNotification) {
                push(Command::DisplayPreferencesChanged);
            }
            #[unsafe(method(choose:))]
            fn choose(&self, sender: &NSMenuItem) {
                let edit_action=match sender.tag(){11=>Some(sel!(cut:)),12=>Some(sel!(copy:)),13=>Some(sel!(paste:)),14=>Some(sel!(selectAll:)),_=>None};
                if let Some(action)=edit_action{let app=NSApplication::sharedApplication(self.mtm());if let Some(first)=app.keyWindow().and_then(|w|w.firstResponder()){let native:bool=unsafe{msg_send![&first,isKindOfClass:objc2::class!(NSTextView)]};if native{unsafe{app.sendAction_to_from(action,Some(&first),None);}return;}}}
                let command = match sender.tag() { 0=>Command::Show,1=>Command::Quit,2=>Command::Save,3=>Command::Feed,4=>Command::Inbox,5=>Command::Archive,6=>Command::Feeds,7=>Command::Magazines,8=>Command::WhatsApp,9=>Command::ImportExport,10=>Command::Search,11=>Command::Cut,12=>Command::Copy,13=>Command::Paste,14=>Command::SelectAll,_=>return };
                push(command);
            }
        }
    );
    extern "C-unwind" fn reopen(_: *mut AnyObject, _: Sel, _: *mut NSApplication, _: Bool) -> Bool {
        push(Command::Show);
        Bool::NO
    }
    extern "C-unwind" fn terminate(_: *mut AnyObject, _: Sel, _: *mut NSApplication) -> usize {
        push(Command::Quit);
        0
    }
    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        HANDLER.with(|slot| {
            if slot.get().is_some() {
                return;
            }
            let handler: Retained<MenuHandler> =
                unsafe { msg_send![MenuHandler::alloc(mtm), init] };
            unsafe {
                NSWorkspace::sharedWorkspace()
                    .notificationCenter()
                    .addObserver_selector_name_object(
                        &handler,
                        sel!(displayOptionsChanged:),
                        Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                        None,
                    );
            }
            let app = NSApplication::sharedApplication(mtm);
            let bar = NSMenu::new(mtm);
            for (title, entries) in [
                (
                    "Read Later",
                    vec![("Show Read Later", "1", 0), ("Quit Read Later", "q", 1)],
                ),
                ("File", vec![("Save Link…", "n", 2), ("Add Feed…", "N", 3)]),
                (
                    "View",
                    vec![
                        ("Inbox", "2", 4),
                        ("Archive", "3", 5),
                        ("Feeds", "4", 6),
                        ("Magazines", "5", 7),
                        ("WhatsApp", "6", 8),
                        ("Import / Export", "7", 9),
                        ("Search", "f", 10),
                    ],
                ),
            ] {
                let top = NSMenuItem::new(mtm);
                top.setTitle(&NSString::from_str(title));
                let menu = NSMenu::new(mtm);
                menu.setTitle(&NSString::from_str(title));
                menu.setAutoenablesItems(false);
                for (label, key, tag) in entries {
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(label),
                            Some(sel!(choose:)),
                            &NSString::from_str(key),
                        )
                    };
                    item.setTag(tag);
                    item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
                    unsafe {
                        item.setTarget(Some(&handler));
                    }
                    menu.addItem(&item);
                }
                top.setSubmenu(Some(&menu));
                bar.addItem(&top);
            }
            let top = NSMenuItem::new(mtm);
            top.setTitle(&NSString::from_str("Edit"));
            let edit = NSMenu::new(mtm);
            edit.setAutoenablesItems(false);
            for (label, key, action) in [
                ("Undo", "z", sel!(undo:)),
                ("Redo", "Z", sel!(redo:)),
                ("Cut", "x", sel!(cut:)),
                ("Copy", "c", sel!(copy:)),
                ("Paste", "v", sel!(paste:)),
                ("Select All", "a", sel!(selectAll:)),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        Some(action),
                        &NSString::from_str(key),
                    )
                };
                if let Some(tag) = match label {
                    "Cut" => Some(11),
                    "Copy" => Some(12),
                    "Paste" => Some(13),
                    "Select All" => Some(14),
                    _ => None,
                } {
                    item.setTag(tag);
                    unsafe {
                        item.setTarget(Some(&handler));
                        item.setAction(Some(sel!(choose:)));
                    }
                }
                edit.addItem(&item);
            }
            top.setSubmenu(Some(&edit));
            bar.insertItem_atIndex(&top, 2);
            // The standard panel reads the bundle's version strings, which the
            // packaging script derives from Cargo.toml and the Git history.
            if let Some(app_menu) = bar.itemAtIndex(0).and_then(|item| item.submenu()) {
                let about = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str("About Read Later"),
                        Some(sel!(orderFrontStandardAboutPanel:)),
                        &NSString::from_str(""),
                    )
                };
                app_menu.insertItem_atIndex(&about, 0);
                app_menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), 1);
            }
            app.setMainMenu(Some(&bar));
            if let Some(delegate) = app.delegate() {
                let object: &AnyObject = AsRef::<AnyObject>::as_ref(&*delegate);
                let class = object.class() as *const AnyClass as *mut AnyClass;
                // Winit owns this delegate throughout this process. Only these application
                // lifecycle selectors are adapted; window/event methods remain Winit's.
                unsafe {
                    objc2::ffi::class_addMethod(
                        class,
                        sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                        (reopen
                            as extern "C-unwind" fn(
                                *mut AnyObject,
                                Sel,
                                *mut NSApplication,
                                Bool,
                            ) -> Bool)
                            .__imp(),
                        c"B@:@B".as_ptr(),
                    );
                    objc2::ffi::class_replaceMethod(
                        class,
                        sel!(applicationShouldTerminate:),
                        (terminate
                            as extern "C-unwind" fn(
                                *mut AnyObject,
                                Sel,
                                *mut NSApplication,
                            ) -> usize)
                            .__imp(),
                        c"Q@:@".as_ptr(),
                    );
                }
            }
            let _ = slot.set(handler);
        });
    }
}
#[cfg(not(target_os = "macos"))]
mod macos {
    pub fn install() {}
}

pub fn clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        unsafe {
            NSPasteboard::generalPasteboard()
                .stringForType(NSPasteboardTypeString)
                .map(|s| s.to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
