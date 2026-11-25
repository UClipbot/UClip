use gtk::{gio, glib, prelude::*};
use tauri::{AppHandle, Manager, PhysicalPosition};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const PLASMA: &str = "org.kde.plasmashell";
const ITEM_PATH: &str = "/UClipTray";
const BUS_DAEMON: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
const ITEM_XML: &str = r#"
<node><interface name="org.kde.StatusNotifierItem">
  <property name="Category" type="s" access="read"/>
  <property name="Id" type="s" access="read"/>
  <property name="Title" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="WindowId" type="u" access="read"/>
  <property name="IconName" type="s" access="read"/>
  <property name="IconPixmap" type="a(iiay)" access="read"/>
  <property name="ItemIsMenu" type="b" access="read"/>
  <property name="Menu" type="o" access="read"/>
  <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
  <method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
  <method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
</interface></node>"#;

struct StatusNotifier {
    _connection: gio::DBusConnection,
    _watcher: gio::SignalSubscriptionId,
}

pub async fn install(app: &AppHandle, rgba: Vec<u8>) -> bool {
    // The watcher protocol has no property for ContextMenu support. Plasma's host
    // forwards it when Menu is /NO_DBUSMENU. Other hosts may only render DBusMenu.
    // X11 also lets us position and focus the resulting popup near the tray icon.
    if crate::desktop::wayland()
        || !std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
            desktop
                .split(':')
                .any(|name| name.eq_ignore_ascii_case("kde"))
        })
    {
        return false;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || register(&app, rgba))
        .await
        .is_ok_and(|registered| registered)
}

fn register(app: &AppHandle, rgba: Vec<u8>) -> bool {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    if plasma_owner(&bus).is_none() || !host_registered(&bus) {
        return false;
    }

    let Ok(node) = gio::DBusNodeInfo::for_xml(ITEM_XML) else {
        return false;
    };
    let Some(item) = node.lookup_interface("org.kde.StatusNotifierItem") else {
        return false;
    };
    let icon: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|px| [px[3], px[0], px[1], px[2]])
        .collect();
    let menu =
        glib::variant::ObjectPath::try_from("/NO_DBUSMENU").expect("valid D-Bus object path");
    let event_app = app.clone();
    let Ok(registration) = bus.register_object(
        ITEM_PATH,
        &item,
        move |connection, sender, _, _, method, parameters, invocation| {
            if plasma_owner(&connection).as_deref() != Some(sender) {
                invocation.return_dbus_error(
                    "org.freedesktop.DBus.Error.AccessDenied",
                    "Tray action requires the Plasma host",
                );
                return;
            }
            match method {
                "Activate" => crate::desktop::show_from_tray(&event_app),
                "ContextMenu" => {
                    let anchor = event_app.cursor_position().ok().or_else(|| {
                        parameters
                            .get::<(i32, i32)>()
                            .map(|(x, y)| PhysicalPosition::new(x.into(), y.into()))
                    });
                    crate::tray_menu::open(&event_app, anchor);
                }
                _ => {}
            }
            invocation.return_value(None);
        },
        move |_, _, _, _, property| match property {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => "uclip".to_variant(),
            "Title" => "UClip".to_variant(),
            "Status" => "Active".to_variant(),
            "WindowId" => 0u32.to_variant(),
            "IconPixmap" => vec![(32i32, 32i32, icon.as_slice())].to_variant(),
            "ItemIsMenu" => false.to_variant(),
            "Menu" => menu.to_variant(),
            "ToolTip" => ("", Vec::<(i32, i32, Vec<u8>)>::new(), "UClip", "").to_variant(),
            _ => "".to_variant(),
        },
        |_, _, _, _, _, _| false,
    ) else {
        return false;
    };
    let Ok(watcher) = register_item(&bus) else {
        let _ = bus.unregister_object(registration);
        return false;
    };
    app.manage(StatusNotifier {
        _connection: bus,
        _watcher: watcher,
    });
    true
}

fn host_registered(bus: &gio::DBusConnection) -> bool {
    bus.call_sync(
        Some(WATCHER),
        WATCHER_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some(&(WATCHER, "IsStatusNotifierHostRegistered").to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        1000,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|reply| reply.get::<(glib::Variant,)>())
    .and_then(|(value,)| value.get::<bool>())
    .unwrap_or(false)
}

fn plasma_owner(bus: &gio::DBusConnection) -> Option<String> {
    bus.call_sync(
        Some(BUS_DAEMON),
        BUS_PATH,
        BUS_DAEMON,
        "GetNameOwner",
        Some(&(PLASMA,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    )
    .ok()?
    .get::<(String,)>()
    .map(|(name,)| name)
}

fn register_item(bus: &gio::DBusConnection) -> Result<gio::SignalSubscriptionId, glib::Error> {
    let parameters = (ITEM_PATH,).to_variant();
    bus.call_sync(
        Some(WATCHER),
        WATCHER_PATH,
        WATCHER,
        "RegisterStatusNotifierItem",
        Some(&parameters),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        1000,
        gio::Cancellable::NONE,
    )?;
    // Re-register when the Watcher restarts.
    Ok(bus.signal_subscribe(
        Some(BUS_DAEMON),
        Some(BUS_DAEMON),
        Some("NameOwnerChanged"),
        Some(BUS_PATH),
        Some(WATCHER),
        gio::DBusSignalFlags::NONE,
        move |connection, _, _, _, _, change| {
            if change
                .get::<(String, String, String)>()
                .is_none_or(|(_, _, owner)| owner.is_empty())
            {
                return;
            }
            connection.call(
                Some(WATCHER),
                WATCHER_PATH,
                WATCHER,
                "RegisterStatusNotifierItem",
                Some(&parameters),
                None,
                gio::DBusCallFlags::NO_AUTO_START,
                1000,
                gio::Cancellable::NONE,
                |_| {},
            );
        },
    ))
}
