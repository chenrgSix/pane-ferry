//! Display-wide DPI belongs only to a publisher-owned X server. Never claim
//! XSETTINGS or write RESOURCE_MANAGER on the user's shared desktop.
use anyhow::{bail, Result};
use x11rb::{
    connection::Connection, protocol::xproto::*, rust_connection::RustConnection,
    wrapper::ConnectionExt as _, CURRENT_TIME,
};

pub struct DisplaySettings {
    pub window: u32,
    pub selection: u32,
    property: u32,
    resources: u32,
    serial: u32,
}

fn encode(scale: u8, serial: u32) -> Vec<u8> {
    let settings = [
        ("Gdk/WindowScalingFactor", i32::from(scale)),
        ("Gdk/UnscaledDPI", 96 * 1024),
        ("Xft/DPI", 96 * 1024 * i32::from(scale)),
    ];
    let mut bytes = vec![0, 0, 0, 0]; // XSETTINGS LSBFirst, independent of host endian.
    bytes.extend(serial.to_le_bytes());
    bytes.extend((settings.len() as u32).to_le_bytes());
    for (name, value) in settings {
        bytes.extend([0, 0]); // integer, unused
        bytes.extend((name.len() as u16).to_le_bytes());
        bytes.extend(name.as_bytes());
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes.extend(serial.to_le_bytes());
        bytes.extend(value.to_le_bytes());
    }
    bytes
}

impl DisplaySettings {
    pub fn start(conn: &RustConnection, root: u32, scale: u8) -> Result<Self> {
        paneferry_protocol::validate_scale(scale)?;
        let atom =
            |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
        let mut settings = Self {
            window: conn.generate_id()?,
            selection: atom(b"_XSETTINGS_S0")?,
            property: atom(b"_XSETTINGS_SETTINGS")?,
            resources: atom(b"RESOURCE_MANAGER")?,
            serial: 0,
        };
        // Refuse an existing manager instead of taking over somebody's display.
        conn.grab_server()?.check()?;
        let claimed = (|| -> Result<()> {
            if conn.get_selection_owner(settings.selection)?.reply()?.owner != 0 {
                bail!("display_settings_unavailable")
            }
            conn.create_window(
                0,
                settings.window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
            conn.set_selection_owner(settings.window, settings.selection, CURRENT_TIME)?
                .check()?;
            Ok(())
        })();
        let released = conn.ungrab_server()?.check();
        claimed?;
        released?;
        settings.update(conn, root, scale)?;
        conn.send_event(
            false,
            root,
            EventMask::STRUCTURE_NOTIFY,
            ClientMessageEvent::new(
                32,
                root,
                atom(b"MANAGER")?,
                [CURRENT_TIME, settings.selection, settings.window, 0, 0],
            ),
        )?
        .check()?;
        conn.flush()?;
        Ok(settings)
    }

    pub fn update(&mut self, conn: &RustConnection, root: u32, scale: u8) -> Result<()> {
        paneferry_protocol::validate_scale(scale)?;
        if conn.get_selection_owner(self.selection)?.reply()?.owner != self.window {
            bail!("display_settings_unavailable")
        }
        self.serial = self.serial.wrapping_add(1);
        conn.change_property8(
            PropMode::REPLACE,
            self.window,
            self.property,
            self.property,
            &encode(scale, self.serial),
        )?
        .check()?;
        // Xft/Qt clients that do not implement XSETTINGS read this standard
        // resource. This server is private; no ~/.Xresources is modified.
        conn.change_property8(
            PropMode::REPLACE,
            root,
            self.resources,
            AtomEnum::STRING,
            format!("Xft.dpi:\t{}\n", 96 * u32::from(scale)).as_bytes(),
        )?
        .check()?;
        conn.flush()?;
        Ok(())
    }
}
