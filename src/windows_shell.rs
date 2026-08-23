use std::path::PathBuf;

pub fn show_context_menu(path: PathBuf) {
    #[cfg(target_os = "windows")]
    std::thread::spawn(move || {
        if let Err(err) = windows_context_menu(&path) {
            eprintln!(
                "Windows shell context menu failed for {}: {err}",
                path.display()
            );
        }
    });

    #[cfg(not(target_os = "windows"))]
    {
        let _ = open::that(path);
    }
}

#[cfg(target_os = "windows")]
fn windows_context_menu(path: &std::path::Path) -> windows::core::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
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
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _com = ComGuard;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let absolute_pidl = SHSimpleIDListFromPath(PCWSTR(wide.as_ptr()));
        if absolute_pidl.is_null() {
            return Err(Error::from_win32());
        }
        let _pidl = PidlGuard(absolute_pidl);

        // Explorer obtains an item's IContextMenu from the item's parent IShellFolder.
        // SHBindToParent also gives us the child PIDL in the parent folder's namespace,
        // which is exactly what GetUIObjectOf expects.
        let mut child_pidl: *const ITEMIDLIST = null();
        let parent: IShellFolder = SHBindToParent(absolute_pidl, Some(&mut child_pidl))?;
        if child_pidl.is_null() {
            return Err(Error::from_win32());
        }

        let owner = GetForegroundWindow();
        let context: IContextMenu = parent.GetUIObjectOf(owner, &[child_pidl], None)?;
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
