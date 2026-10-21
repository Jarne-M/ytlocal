//! Windows toast notifications. Best effort: failures are ignored, the app
//! always shows an in-app message as well.

pub fn toast(title: &str, body: &str) {
    #[cfg(windows)]
    {
        use tauri_winrt_notification::Toast;
        let _ = Toast::new(Toast::POWERSHELL_APP_ID)
            .title(title)
            .text1(body)
            .show();
    }
    #[cfg(not(windows))]
    let _ = (title, body);
}
