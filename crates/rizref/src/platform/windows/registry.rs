//! File-type registration: `.rizref` → `RizRef.Board`, its icon, the open
//! verb and the Explorer thumbnail provider.
//!
//! `rizref register` writes the keys for the current user under
//! `HKCU\Software\Classes` — no administrator rights, which is what a portable
//! exe wants. `rizref register --machine` writes the same tree under
//! `HKLM\Software\Classes` for an installer running elevated. `unregister`
//! removes exactly what `register` wrote. The registry layout is built by the
//! pure [`plan`], which the tests pin down, and applied by [`apply`].
//!
//! The thumbnail provider (`rizref_thumb.dll`) is registered only when it
//! sits next to the exe: a bare portable `rizref.exe` still gets its file
//! type, icon and open verb, just no Explorer previews.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::PCWSTR;

/// The ProgID boards are registered under.
pub const PROG_ID: &str = "RizRef.Board";

/// The file extension, with its dot.
pub const EXTENSION: &str = ".rizref";

/// The thumbnail DLL's file name, looked for next to the exe.
pub const THUMB_DLL: &str = "rizref_thumb.dll";

/// The thumbnail provider's class id: `rizref_thumb::CLSID_STRING`, repeated
/// here so the exe does not link the DLL's crate (and its COM exports); a
/// test keeps the two equal.
pub const THUMB_CLSID: &str = "{6F1A3C52-8E4B-4D2A-9C7E-3B5D2F18A4C9}";

/// The shell's thumbnail-handler category (`IThumbnailProvider`), mirrored
/// from `rizref_thumb` and checked by the same test.
pub const THUMBNAIL_HANDLER_CATEGORY: &str = "{e357fccd-a995-4576-b01f-234630154e96}";

/// Where the keys go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `HKCU\Software\Classes`: this user, no elevation.
    User,
    /// `HKLM\Software\Classes`: every user; needs an elevated installer.
    Machine,
}

/// One registry value to write: `key` relative to `Software\Classes`, a value
/// name (`None` is the key's default value) and string data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// Key path below `Software\Classes`.
    pub key: String,
    /// Value name; `None` for the default value.
    pub name: Option<String>,
    /// String data.
    pub data: String,
}

fn value(key: impl Into<String>, name: Option<&str>, data: impl Into<String>) -> Value {
    Value {
        key: key.into(),
        name: name.map(str::to_owned),
        data: data.into(),
    }
}

use crate::platform::register::{DOCUMENT_ICON_INDEX, MIME_TYPE};

/// Every value `register` writes for an exe at `exe`, plus the thumbnail
/// provider when `thumb_dll` is given.
pub fn plan(exe: &Path, thumb_dll: Option<&Path>) -> Vec<Value> {
    let exe = exe.display().to_string();
    let open = format!("\"{exe}\" \"%1\"");
    let mut values = vec![
        value(EXTENSION, None, PROG_ID),
        value(EXTENSION, Some("Content Type"), MIME_TYPE),
        value(EXTENSION, Some("PerceivedType"), "document"),
        value(format!("{EXTENSION}\\OpenWithProgids"), Some(PROG_ID), ""),
        value(PROG_ID, None, "RizRef board"),
        value(PROG_ID, Some("FriendlyTypeName"), "RizRef board"),
        // The second icon group in the exe: the document page, not the app
        // mark (`build.rs` embeds the app icon first and the document second).
        value(
            format!("{PROG_ID}\\DefaultIcon"),
            None,
            format!("\"{exe}\",{DOCUMENT_ICON_INDEX}"),
        ),
        value(format!("{PROG_ID}\\shell"), None, "open"),
        value(format!("{PROG_ID}\\shell\\open"), None, "Open with RizRef"),
        value(
            format!("{PROG_ID}\\shell\\open\\command"),
            None,
            open.clone(),
        ),
        value("Applications\\rizref.exe\\shell\\open\\command", None, open),
        value(
            "Applications\\rizref.exe\\SupportedTypes",
            Some(EXTENSION),
            "",
        ),
        value(
            "Applications\\rizref.exe",
            Some("FriendlyAppName"),
            "RizRef",
        ),
    ];
    if let Some(dll) = thumb_dll {
        let clsid = THUMB_CLSID;
        let category = THUMBNAIL_HANDLER_CATEGORY;
        values.extend([
            value(format!("CLSID\\{clsid}"), None, "RizRef thumbnail provider"),
            value(
                format!("CLSID\\{clsid}\\InprocServer32"),
                None,
                dll.display().to_string(),
            ),
            value(
                format!("CLSID\\{clsid}\\InprocServer32"),
                Some("ThreadingModel"),
                "Apartment",
            ),
            // Microsoft's guidance puts the handler under the extension; the
            // ProgID copy covers shells that resolve through it.
            value(format!("{EXTENSION}\\ShellEx\\{category}"), None, clsid),
            value(format!("{PROG_ID}\\ShellEx\\{category}"), None, clsid),
        ]);
    }
    values
}

/// The key trees `unregister` deletes. The extension key is removed whole: it
/// holds nothing but what `plan` wrote.
pub fn owned_trees() -> Vec<String> {
    vec![
        EXTENSION.to_owned(),
        PROG_ID.to_owned(),
        "Applications\\rizref.exe".to_owned(),
        format!("CLSID\\{THUMB_CLSID}"),
    ]
}

/// The thumbnail DLL next to `exe`, if it is there.
pub fn thumb_dll_beside(exe: &Path) -> Option<PathBuf> {
    let dll = exe.parent()?.join(THUMB_DLL);
    dll.is_file().then_some(dll)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn root(scope: Scope) -> HKEY {
    match scope {
        Scope::User => HKEY_CURRENT_USER,
        Scope::Machine => HKEY_LOCAL_MACHINE,
    }
}

/// Writes `values` under `Software\Classes` of `scope` and tells the shell
/// that associations changed.
// SAFETY: every pointer passed is a NUL-terminated buffer or byte slice owned
// by this function for the duration of the call; opened keys are closed.
#[allow(unsafe_code)]
pub fn apply(scope: Scope, values: &[Value]) -> Result<(), String> {
    for entry in values {
        let key_path = wide(&format!("Software\\Classes\\{}", entry.key));
        let mut key = HKEY::default();
        unsafe {
            let status = RegCreateKeyExW(
                root(scope),
                PCWSTR(key_path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            );
            if status != ERROR_SUCCESS {
                return Err(format!("cannot create {}: error {}", entry.key, status.0));
            }
            let name = entry.name.as_deref().map(wide);
            let data: Vec<u8> = wide(&entry.data)
                .iter()
                .flat_map(|unit| unit.to_le_bytes())
                .collect();
            let status = RegSetValueExW(
                key,
                name.as_ref()
                    .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr())),
                None,
                REG_SZ,
                Some(&data),
            );
            let _ = RegCloseKey(key);
            if status != ERROR_SUCCESS {
                return Err(format!("cannot write {}: error {}", entry.key, status.0));
            }
        }
    }
    notify_shell();
    Ok(())
}

/// Deletes every tree `register` owns under `scope`. Missing keys are fine.
// SAFETY: as for `apply`.
#[allow(unsafe_code)]
pub fn remove(scope: Scope) -> Result<(), String> {
    for tree in owned_trees() {
        let path = wide(&format!("Software\\Classes\\{tree}"));
        let status = unsafe { RegDeleteTreeW(root(scope), PCWSTR(path.as_ptr())) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            return Err(format!("cannot delete {tree}: error {}", status.0));
        }
    }
    notify_shell();
    Ok(())
}

/// Tells Explorer to re-read file associations (icons, verbs, thumbnails).
// SAFETY: `SHCNE_ASSOCCHANGED` takes no items; both item pointers are null.
#[allow(unsafe_code)]
fn notify_shell() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// `rizref register [--machine]` / `rizref unregister [--machine]`.
/// Exit code 0 on success, 1 on a registry failure, 2 on a bad argument.
pub fn run(register: bool, args: &[String]) -> std::process::ExitCode {
    let scope = match args {
        [] => Scope::User,
        [flag] if flag == "--machine" => Scope::Machine,
        _ => {
            eprintln!(
                "usage: rizref {} [--machine]",
                if register { "register" } else { "unregister" }
            );
            return std::process::ExitCode::from(2);
        }
    };
    let result = if register {
        match std::env::current_exe() {
            Ok(exe) => {
                let dll = thumb_dll_beside(&exe);
                let outcome = apply(scope, &plan(&exe, dll.as_deref()));
                if outcome.is_ok() {
                    println!("registered {EXTENSION} for {} ({scope:?})", exe.display());
                    match &dll {
                        Some(dll) => println!("thumbnail provider: {}", dll.display()),
                        None => {
                            println!("no {THUMB_DLL} next to the exe: Explorer thumbnails stay off")
                        }
                    }
                }
                outcome
            }
            Err(error) => Err(format!("cannot locate rizref.exe: {error}")),
        }
    } else {
        let outcome = remove(scope);
        if outcome.is_ok() {
            println!("unregistered {EXTENSION} ({scope:?})");
        }
        outcome
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rizref: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EXTENSION, PROG_ID, THUMB_CLSID, THUMBNAIL_HANDLER_CATEGORY, owned_trees, plan};
    use std::path::Path;

    #[test]
    fn the_mirrored_class_ids_match_the_thumbnail_dll() {
        assert_eq!(THUMB_CLSID, rizref_thumb::CLSID_STRING);
        assert_eq!(
            THUMBNAIL_HANDLER_CATEGORY,
            rizref_thumb::THUMBNAIL_HANDLER_CATEGORY
        );
    }

    fn find<'a>(values: &'a [super::Value], key: &str, name: Option<&str>) -> Option<&'a str> {
        values
            .iter()
            .find(|v| v.key == key && v.name.as_deref() == name)
            .map(|v| v.data.as_str())
    }

    #[test]
    fn the_extension_points_at_the_prog_id_with_icon_and_open_verb() {
        let exe = Path::new(r"C:\Apps\RizRef\rizref.exe");

        let values = plan(exe, None);

        assert_eq!(find(&values, EXTENSION, None), Some(PROG_ID));
        assert_eq!(
            find(&values, &format!("{PROG_ID}\\DefaultIcon"), None),
            Some(r#""C:\Apps\RizRef\rizref.exe",1"#),
            "the document icon, not the app icon"
        );
        assert_eq!(
            find(&values, &format!("{PROG_ID}\\shell\\open\\command"), None),
            Some(r#""C:\Apps\RizRef\rizref.exe" "%1""#)
        );
        assert!(
            !values.iter().any(|v| v.key.contains("CLSID")),
            "no thumbnail DLL, no provider keys"
        );
    }

    #[test]
    fn the_thumbnail_provider_is_registered_when_its_dll_is_present() {
        let exe = Path::new(r"C:\Apps\RizRef\rizref.exe");
        let dll = Path::new(r"C:\Apps\RizRef\rizref_thumb.dll");

        let values = plan(exe, Some(dll));

        let clsid = THUMB_CLSID;
        let inproc = format!("CLSID\\{clsid}\\InprocServer32");
        assert_eq!(
            find(&values, &inproc, None),
            Some(r"C:\Apps\RizRef\rizref_thumb.dll")
        );
        assert_eq!(
            find(&values, &inproc, Some("ThreadingModel")),
            Some("Apartment")
        );
        assert_eq!(
            find(
                &values,
                &format!("{EXTENSION}\\ShellEx\\{}", THUMBNAIL_HANDLER_CATEGORY),
                None
            ),
            Some(clsid)
        );
    }

    #[test]
    fn unregister_removes_every_tree_register_writes_into() {
        let values = plan(Path::new("x.exe"), Some(Path::new("y.dll")));
        let trees = owned_trees();

        for value in &values {
            assert!(
                trees
                    .iter()
                    .any(|tree| value.key == *tree || value.key.starts_with(&format!("{tree}\\"))),
                "{} is written but never removed",
                value.key
            );
        }
    }
}
