use std::path::{Path, PathBuf};

/// Returns the query that should be sent to Everything for a selected file.
///
/// Normal searches use the complete file name. Ctrl-click searches keep only
/// the longest contiguous ASCII digit run, which is useful for names such as
/// `shutter_294918522.jpg`.
pub fn everything_query(path: &Path, numeric_only: bool) -> Result<String, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("Cannot determine the file name for {}", path.display()))?;

    if !numeric_only {
        return Ok(file_name.to_owned());
    }

    let numeric_source = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(file_name);
    let mut best = "";
    let mut current_start = None;
    for (index, character) in numeric_source.char_indices() {
        if character.is_ascii_digit() {
            current_start.get_or_insert(index);
        } else if let Some(start) = current_start.take() {
            let run = &numeric_source[start..index];
            if run.len() > best.len() {
                best = run;
            }
        }
    }
    if let Some(start) = current_start {
        let run = &numeric_source[start..];
        if run.len() > best.len() {
            best = run;
        }
    }

    if best.is_empty() {
        Err(format!(
            "No number was found in the file name `{file_name}`"
        ))
    } else {
        Ok(best.to_owned())
    }
}

/// Find an installed Everything executable without requiring it to be on PATH.
/// Everything's standard Windows installer uses one of the common roots below;
/// PATH is also checked for portable or custom installations.
#[cfg(target_os = "windows")]
fn everything_executable() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let mut add_root = |root: Option<std::ffi::OsString>| {
        if let Some(root) = root {
            let root = PathBuf::from(root);
            for directory in ["Everything", "Everything 1.4", "Everything 1.5"] {
                candidates.push(root.join(directory).join("Everything.exe"));
                candidates.push(root.join(directory).join("Everything64.exe"));
            }
        }
    };
    add_root(std::env::var_os("ProgramFiles"));
    add_root(std::env::var_os("ProgramFiles(x86)"));
    add_root(std::env::var_os("LOCALAPPDATA"));
    add_root(std::env::var_os("APPDATA"));

    if let Ok(path) = std::env::var("PATH") {
        for directory in std::env::split_paths(&path) {
            candidates.push(directory.join("Everything.exe"));
            candidates.push(directory.join("Everything64.exe"));
        }
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("Everything.exe"));
            candidates.push(directory.join("Everything64.exe"));
        }
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(not(target_os = "windows"))]
fn everything_executable() -> Option<PathBuf> {
    None
}

pub fn everything_is_available() -> bool {
    everything_executable().is_some()
}

pub fn search_in_everything(path: PathBuf, numeric_only: bool) -> Result<(), String> {
    let query = everything_query(&path, numeric_only)?;

    #[cfg(target_os = "windows")]
    {
        let executable = everything_executable().ok_or_else(|| {
            "Everything is not installed or its executable could not be found".to_owned()
        })?;
        std::process::Command::new(&executable)
            .arg("-search")
            .arg(&query)
            .spawn()
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "Cannot start Everything for `{query}` using {}: {error}",
                    executable.display()
                )
            })
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = query;
        Err("Search by Everything is currently available only on Windows".to_owned())
    }
}

pub fn ctrl_v_is_down() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};

        const VK_V: i32 = 0x56;
        let control = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16;
        let v = unsafe { GetAsyncKeyState(VK_V) } as u16;
        control & 0x8000 != 0 && v & 0x8000 != 0
    }

    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

pub fn copy_file(path: PathBuf) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        invoke_shell_verb(&path, b"copy\0").map_err(|err| {
            format!(
                "Cannot copy {} to the Windows clipboard: {err}",
                path.display()
            )
        })
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Copy file is currently available only on Windows".to_owned())
    }
}

pub fn open_in_photoshop(path: PathBuf) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows_open_in_photoshop(&path)
            .map_err(|err| format!("Cannot open {} in Adobe Photoshop: {err}", path.display()))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Open in Photoshop is currently available only on Windows".to_owned())
    }
}

pub fn show_in_explorer(path: PathBuf) {
    #[cfg(target_os = "windows")]
    {
        if let Err(err) = windows_show_in_explorer(&path) {
            eprintln!("Cannot show {} in Explorer: {err}", path.display());
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(parent) = path.parent() {
            let _ = open::that(parent);
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_show_in_explorer(path: &std::path::Path) -> windows::core::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Error, PCWSTR};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        ILCreateFromPathW, ILFindLastID, ILFree, SHOpenFolderAndSelectItems,
    };

    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    let Some(parent) = path.parent() else {
        return Err(Error::from_win32());
    };
    let folder_wide: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
    let file_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _com = ComGuard;

        let folder_pidl = ILCreateFromPathW(PCWSTR(folder_wide.as_ptr()));
        if folder_pidl.is_null() {
            return Err(Error::from_win32());
        }
        let file_pidl = ILCreateFromPathW(PCWSTR(file_wide.as_ptr()));
        if file_pidl.is_null() {
            ILFree(Some(folder_pidl));
            return Err(Error::from_win32());
        }

        let child_pidl = ILFindLastID(file_pidl);
        let result = if child_pidl.is_null() {
            Err(Error::from_win32())
        } else {
            SHOpenFolderAndSelectItems(folder_pidl, Some(&[child_pidl]), 0)
        };
        ILFree(Some(file_pidl));
        ILFree(Some(folder_pidl));
        result
    }
}

pub fn show_context_menu(path: PathBuf) {
    #[cfg(target_os = "windows")]
    {
        // Keep Shell menu creation on the GUI thread that owns the foreground window.
        // Shell context-menu handlers can depend on the owner thread's window/message
        // loop; running the whole IContextMenu/TrackPopupMenuEx sequence on a detached
        // worker thread can make the native menu silently fail to appear.
        if let Err(err) = windows_context_menu(&path) {
            eprintln!(
                "Windows shell context menu failed for {}: {err}",
                path.display()
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = open::that(path);
    }
}

#[cfg(target_os = "windows")]
fn windows_context_menu(path: &std::path::Path) -> windows::core::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows::core::{Error, PCSTR, PCWSTR};
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToParent, SHSimpleIDListFromPath, CMF_NORMAL,
        CMINVOKECOMMANDINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetCursorPos, GetForegroundWindow, TrackPopupMenuEx,
        SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    };

    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct PidlGuard(*mut ITEMIDLIST);
    impl Drop for PidlGuard {
        fn drop(&mut self) {
            unsafe {
                CoTaskMemFree(Some(self.0.cast::<c_void>()));
            }
        }
    }

    unsafe {
        // This now runs on the eframe UI thread. Keeping COM, the owner HWND and the
        // TrackPopupMenuEx modal loop on the same thread is important for Shell menu
        // handlers that expect their owner window to participate in message dispatch.
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _com = ComGuard;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let absolute_pidl = SHSimpleIDListFromPath(PCWSTR(wide.as_ptr()));
        if absolute_pidl.is_null() {
            return Err(Error::from_win32());
        }
        let _pidl = PidlGuard(absolute_pidl);

        // Explorer obtains an item's IContextMenu from the item's parent IShellFolder.
        // SHBindToParent returns the child PIDL in the parent's namespace; that child PIDL
        // is what IShellFolder::GetUIObjectOf expects.
        let mut child_pidl: *mut ITEMIDLIST = null_mut();
        let parent: IShellFolder = SHBindToParent(absolute_pidl, Some(&mut child_pidl))?;
        if child_pidl.is_null() {
            return Err(Error::from_win32());
        }

        let owner = GetForegroundWindow();
        let child = child_pidl as *const ITEMIDLIST;
        let context: IContextMenu = parent.GetUIObjectOf(owner, &[child], None)?;
        let menu = CreatePopupMenu()?;

        let menu_result = (|| -> windows::core::Result<()> {
            const FIRST_COMMAND: u32 = 1;
            context
                .QueryContextMenu(menu, 0, FIRST_COMMAND, 0x7fff, CMF_NORMAL)
                .ok()?;

            let mut point = POINT::default();
            GetCursorPos(&mut point)?;
            let selected = TrackPopupMenuEx(
                menu,
                (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                point.x,
                point.y,
                owner,
                None,
            );
            let command = selected.0 as u32;
            if command >= FIRST_COMMAND {
                let invoke = CMINVOKECOMMANDINFO {
                    cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                    hwnd: owner,
                    lpVerb: PCSTR((command - FIRST_COMMAND) as usize as *const u8),
                    nShow: SW_SHOWNORMAL.0,
                    ..Default::default()
                };
                context.InvokeCommand(&invoke)?;
            }
            Ok(())
        })();

        let _ = DestroyMenu(menu);
        menu_result
    }
}

#[cfg(target_os = "windows")]
fn invoke_shell_verb(path: &std::path::Path, verb: &'static [u8]) -> windows::core::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows::core::{Error, PCSTR, PCWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToParent, SHSimpleIDListFromPath, CMINVOKECOMMANDINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_SHOWNORMAL};

    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct PidlGuard(*mut ITEMIDLIST);
    impl Drop for PidlGuard {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(Some(self.0.cast::<c_void>())) };
        }
    }

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _com = ComGuard;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let absolute_pidl = SHSimpleIDListFromPath(PCWSTR(wide.as_ptr()));
        if absolute_pidl.is_null() {
            return Err(Error::from_win32());
        }
        let _pidl = PidlGuard(absolute_pidl);

        let mut child_pidl: *mut ITEMIDLIST = null_mut();
        let parent: IShellFolder = SHBindToParent(absolute_pidl, Some(&mut child_pidl))?;
        if child_pidl.is_null() {
            return Err(Error::from_win32());
        }

        let owner = GetForegroundWindow();
        let child = child_pidl as *const ITEMIDLIST;
        let context: IContextMenu = parent.GetUIObjectOf(owner, &[child], None)?;
        let invoke = CMINVOKECOMMANDINFO {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
            hwnd: owner,
            lpVerb: PCSTR(verb.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        context.InvokeCommand(&invoke)
    }
}

#[cfg(target_os = "windows")]
fn windows_open_in_photoshop(path: &std::path::Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_SHOWNORMAL};

    let executable: Vec<u16> = "Photoshop.exe".encode_utf16().chain(Some(0)).collect();
    let parameters: Vec<u16> = format!("\"{}\"", path.display())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            Some(GetForegroundWindow()),
            PCWSTR::null(),
            PCWSTR(executable.as_ptr()),
            PCWSTR(parameters.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let status = result.0 as isize;
    if status > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not start Photoshop (ShellExecute error {status}). Is Photoshop installed?"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::everything_query;
    use std::path::Path;

    #[test]
    fn everything_query_uses_file_name_for_normal_search() {
        assert_eq!(
            everything_query(Path::new(r"C:\Images\shutter_294918522.jpg"), false).unwrap(),
            "shutter_294918522.jpg"
        );
    }

    #[test]
    fn everything_query_extracts_longest_number_for_ctrl_search() {
        assert_eq!(
            everything_query(Path::new(r"C:\Images\shutter_294918522.jpg"), true).unwrap(),
            "294918522"
        );
    }

    #[test]
    fn everything_query_rejects_numeric_search_without_digits() {
        let error = everything_query(Path::new(r"C:\Images\texture.jpg"), true).unwrap_err();
        assert!(error.contains("No number"));
    }

    #[test]
    fn everything_query_ignores_digits_in_the_extension() {
        assert_eq!(
            everything_query(Path::new(r"C:\Images\shutter_42.jpg2000"), true).unwrap(),
            "42"
        );
    }
}
