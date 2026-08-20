use std::path::PathBuf;

pub fn show_context_menu(path: PathBuf) {
    #[cfg(target_os = "windows")]
    std::thread::spawn(move || {
        if let Err(err) = windows_context_menu(&path) {
            eprintln!("Windows shell context menu failed for {}: {err}", path.display());
        }
    });

    #[cfg(not(target_os = "windows"))]
    {
        let _ = open::that(path);
    }
}

#[cfg(target_os = "windows")]
fn windows_context_menu(path: &std::path::Path) -> windows::core::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellItem, CMINVOKECOMMANDINFO, CMF_NORMAL, BHID_SFUIObject,
        SHCreateItemFromParsingName,
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

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _com = ComGuard;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
        let context: IContextMenu = item.BindToHandler(None, &BHID_SFUIObject)?;
        let menu = CreatePopupMenu()?;

        let menu_result = (|| -> windows::core::Result<()> {
            const FIRST_COMMAND: u32 = 1;
            context
                .QueryContextMenu(menu, 0, FIRST_COMMAND, 0x7fff, CMF_NORMAL)
                .ok()?;

            let mut point = POINT::default();
            GetCursorPos(&mut point)?;
            let owner = GetForegroundWindow();
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
